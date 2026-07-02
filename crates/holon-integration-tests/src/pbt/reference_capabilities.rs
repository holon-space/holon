//! Phase 2 — blanket impls of `holon_pbt_core::capabilities::*` on
//! [`ReferenceState`].
//!
//! Translates between the capability traits' stringly-typed `EntityUri`
//! surface and `ReferenceState`'s `EntityUri`-based internals. Zero
//! behaviour change: every method delegates to an existing
//! `ReferenceState` field or method.
//!
//! When the wide PBT's transitions migrate to capability bounds (Phase 3),
//! they pick these impls up automatically — no code in the transition
//! changes beyond the trait-bound `where` clauses.
//!
//! ## Boundary translation: `EntityUri` ↔ `EntityUri`
//!
//! - Read methods returning ids: produce `String` via `EntityUri::as_str().to_string()`.
//! - Read methods taking ids: parse via `EntityUri::from_str` and propagate `None`
//!   on failure (a malformed id can't match anything in the block tree).
//! - Write methods taking ids: parse + `.expect()` — wide PBT only ever
//!   sees valid `EntityUri`s, so a parse failure is a programmer error.

use std::collections::BTreeSet;
use std::sync::Arc;

use holon_api::Region;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::{
    CapCursor, CapRegion, RefBackend, RefBlockTree, RefBlockTreeMut, RefDocuments, RefDocumentsMut,
    RefEditorMirror, RefEditorMirrorMut, RefFocus, RefFocusMut, RefFocusRoots, RefGlobalFocus,
    RefHistory, RefHistoryMut, RefLayout, RefLifecycle, RefNavHistory, RefNavHistoryMut, RefPeers,
    RefPeersMut, RefRender, RefRenderMut, RefTaskState, RefToggles, RefTogglesMut, RefWatches,
    WatchRow,
};

use super::peer_ops::PeerBlock;
use super::reference_state::PeerRefState;
use super::state_machine::{merge_peer_blocks_into_primary, refresh_peer_baseline};

use super::reference_state::{CursorPosition, ReferenceState, Resolved};

// ─── Helpers ──────────────────────────────────────────────────────────
//
// `CapBlockId` is now `holon_api::EntityUri` (the capability surface uses
// the real domain type), so the former `EntityUri ↔ String` boundary
// translation collapses to identity. These thin wrappers are kept so the
// per-method call sites below don't all have to change shape.

fn cap_id(uri: &EntityUri) -> EntityUri {
    uri.clone()
}

fn cap_id_set(uris: BTreeSet<EntityUri>) -> BTreeSet<EntityUri> {
    uris
}

/// Identity: a `CapBlockId` already *is* an `EntityUri`.
fn parse_id(id: &EntityUri) -> Option<EntityUri> {
    Some(id.clone())
}

/// Identity: a `CapBlockId` already *is* an `EntityUri`.
fn parse_id_must(id: &EntityUri) -> EntityUri {
    id.clone()
}

fn from_cap_region(r: CapRegion) -> Region {
    // CapRegion::Sidebar maps to LeftSidebar (the primary sidebar in wide PBT).
    // Pure slice impls use Single (no region distinction).
    match r {
        CapRegion::Main | CapRegion::Single => Region::Main,
        CapRegion::Sidebar => Region::LeftSidebar,
    }
}

// ─── RefLifecycle ─────────────────────────────────────────────────────

impl RefLifecycle for ReferenceState {
    fn app_started(&self) -> bool {
        self.action.app_started
    }
    fn is_properly_setup(&self) -> bool {
        self.is_properly_setup()
    }
    fn enable_loro(&self) -> bool {
        self.wiring
            .has_storage(holon_pbt_core::StorageAdapter::Loro)
    }
    fn has_editor_buffer(&self) -> bool {
        ReferenceState::has_editor_buffer(self)
    }
    fn renders_block_interactively(&self, block_id: &EntityUri) -> bool {
        ReferenceState::renders_block_interactively(self, block_id)
    }
    fn last_transition_kind(&self) -> Option<&'static str> {
        self.action.last_transition_kind
    }
}

// ─── RefBlockTree ─────────────────────────────────────────────────────

impl RefBlockTree for ReferenceState {
    fn block_content(&self, id: &EntityUri) -> Option<&str> {
        let uri = parse_id(id)?;
        self.domain
            .block_state
            .blocks
            .get(&uri)
            .map(|b| b.content.as_str())
    }

    fn is_text_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain
            .block_state
            .blocks
            .get(&uri)
            .is_some_and(|b| b.content_type == holon_api::ContentType::Text)
    }

    fn main_editable_descendants(&self) -> Vec<EntityUri> {
        ReferenceState::main_editable_descendants(self)
            .iter()
            .map(cap_id)
            .collect()
    }

    fn focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri> {
        cap_id_set(self.expected_focus_root_ids(from_cap_region(region)))
    }

    fn previous_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let uri = parse_id(id)?;
        ReferenceState::previous_sibling(self, &uri)
            .as_ref()
            .map(cap_id)
    }

    fn next_sibling(&self, id: &EntityUri) -> Option<EntityUri> {
        let uri = parse_id(id)?;
        ReferenceState::next_sibling(self, &uri)
            .as_ref()
            .map(cap_id)
    }

    fn parent_of(&self, id: &EntityUri) -> Option<EntityUri> {
        let uri = parse_id(id)?;
        let b = self.domain.block_state.blocks.get(&uri)?;
        if b.parent_id.is_no_parent() || b.parent_id.is_sentinel() {
            None
        } else {
            Some(cap_id(&b.parent_id))
        }
    }

    fn grandparent(&self, id: &EntityUri) -> Option<EntityUri> {
        let uri = parse_id(id)?;
        ReferenceState::grandparent(self, &uri).as_ref().map(cap_id)
    }

    fn sorted_children(&self, parent: &EntityUri) -> Vec<EntityUri> {
        let Some(uri) = parse_id(parent) else {
            return vec![];
        };
        ReferenceState::sorted_children_of(self, &uri)
            .into_iter()
            .map(|b| cap_id(&b.id))
            .collect()
    }

    fn is_descendant_of_any(&self, id: &EntityUri, ancestors: &BTreeSet<EntityUri>) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        let ancestor_uris: BTreeSet<EntityUri> = ancestors.iter().filter_map(parse_id).collect();
        ReferenceState::is_descendant_of_any(self, &uri, &ancestor_uris)
    }

    fn is_layout_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.layout_blocks.contains(&uri)
    }

    fn is_focusable(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.layout_blocks.is_focusable(&uri)
    }

    fn is_no_content_update(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.layout_blocks.render_source_ids.contains(&uri)
            || self.domain.layout_blocks.query_source_ids.contains(&uri)
            || self.domain.profile_block_ids.contains(&uri)
    }

    fn is_order_exempt_sibling(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.block_state.blocks.get(&uri).is_some_and(|b| {
            matches!(
                b.content_type,
                holon_api::ContentType::Source | holon_api::ContentType::Image
            )
        })
    }

    fn is_page_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain
            .block_state
            .blocks
            .get(&uri)
            .is_some_and(|b| b.is_page())
    }

    fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri> {
        self.domain
            .block_state
            .blocks
            .keys()
            .filter(|uri| {
                let is_seed = self
                    .domain
                    .block_state
                    .block_documents
                    .get(uri)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                !is_seed
            })
            .map(cap_id)
            .collect()
    }

    fn block_exists(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.block_state.blocks.contains_key(&uri)
    }

    fn is_immutable(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain.layout_blocks.is_immutable(&uri)
    }
}

// ─── RefBlockTreeMut ──────────────────────────────────────────────────

impl RefBlockTreeMut for ReferenceState {
    fn push_undo_snapshot(&mut self) {
        ReferenceState::push_undo_snapshot(self);
    }

    fn set_block_content(&mut self, id: &EntityUri, text: &str) {
        let uri = parse_id_must(id);
        if let Some(b) = self.domain.block_state.blocks.get_mut(&uri) {
            // Editor-commit write path: normalize exactly like prod's
            // `SqlOperationProvider::trimmed_content`, mirroring the
            // inherent `commit_active_editor_if_changed`. The generic
            // pbt-core commit helper writes through here, so both commit
            // paths now share one normalization.
            b.content = super::types::normalize_content_for_org_roundtrip(text, b.content_type);
        }
    }

    fn split_block(&mut self, id: &EntityUri, position: usize) -> EntityUri {
        let uri = parse_id_must(id);
        let new_uri = ReferenceState::split_block(self, &uri, position);
        cap_id(&new_uri)
    }

    fn join_block(&mut self, id: &EntityUri) -> usize {
        let uri = parse_id_must(id);
        ReferenceState::join_block(self, &uri)
    }

    fn indent(&mut self, id: &EntityUri) {
        // Mirror `transitions/indent.rs::apply_to_ref`:
        //   prev = previous_sibling
        //   after = sorted_children_of(prev).last().id
        //   move_block(id, prev, after)
        let uri = parse_id_must(id);
        let prev = ReferenceState::previous_sibling(self, &uri)
            .expect("indent: previous sibling required");
        let after = ReferenceState::sorted_children_of(self, &prev)
            .last()
            .map(|b| b.id.clone());
        ReferenceState::move_block(self, &uri, prev, after.as_ref());
    }

    fn outdent(&mut self, id: &EntityUri) {
        let uri = parse_id_must(id);
        ReferenceState::outdent_block(self, &uri);
    }

    fn move_block(&mut self, id: &EntityUri, new_parent: EntityUri, after: Option<&EntityUri>) {
        let uri = parse_id_must(id);
        let parent_uri = parse_id_must(&new_parent);
        let after_uri = after.map(parse_id_must);
        ReferenceState::move_block(self, &uri, parent_uri, after_uri.as_ref());
    }

    fn swap_siblings(&mut self, a: &EntityUri, b: &EntityUri) {
        let a_uri = parse_id_must(a);
        let b_uri = parse_id_must(b);
        ReferenceState::swap_sequence(self, &a_uri, &b_uri);
    }

    fn set_edge_field(&mut self, id: &EntityUri, update: &holon_api::EdgeFieldUpdate) {
        let uri = parse_id_must(id);
        let block = self
            .domain
            .block_state
            .blocks
            .get_mut(&uri)
            .expect("SetEdgeField: subject block must exist (precondition)");
        // `is_page` is computed from `tags` on read, so there is no cached
        // state to keep in sync.
        match update {
            holon_api::EdgeFieldUpdate::Tags(tags) => block.tags = tags.clone(),
            holon_api::EdgeFieldUpdate::Requires(reqs) => block.requires = reqs.clone(),
        }
    }
}

// ─── RefEditorMirror ──────────────────────────────────────────────────

impl RefEditorMirror for ReferenceState {
    fn active_editor_block(&self) -> Option<EntityUri> {
        self.ui
            .tab
            .active_editor
            .as_ref()
            .map(|e| cap_id(&e.block_id))
    }

    fn active_editor_text(&self) -> Option<&str> {
        self.ui
            .tab
            .active_editor
            .as_ref()
            .map(|e| e.in_memory_content.as_str())
    }

    fn active_editor_cursor(&self) -> Option<usize> {
        self.ui.tab.active_editor.as_ref().map(|e| e.cursor_byte)
    }

    fn active_editor_dirty(&self) -> bool {
        self.ui.tab.active_editor.as_ref().is_some_and(|e| e.dirty)
    }
}

// ─── RefEditorMirrorMut ───────────────────────────────────────────────

impl RefEditorMirrorMut for ReferenceState {
    fn type_chars(&mut self, text: &str) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.type_chars(text);
        }
    }

    fn delete_backward(&mut self, count: usize) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.delete_backward(count);
        }
    }

    fn move_cursor(&mut self, byte_position: usize) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.move_cursor(byte_position);
        }
    }

    fn mark_active_editor_committed(&mut self) {
        if let Some(editor) = self.ui.tab.active_editor.as_mut() {
            editor.dirty = false;
        }
    }
}

// ─── RefFocus ─────────────────────────────────────────────────────────

impl RefFocus for ReferenceState {
    fn expected_focus_root_rows(&self) -> Vec<(String, Vec<String>)> {
        Region::ALL
            .iter()
            .map(|region| {
                let roots = self
                    .expected_focus_root_ids(*region)
                    .into_iter()
                    .map(|u| u.as_str().to_string())
                    .collect();
                (region.as_str().to_string(), roots)
            })
            .collect()
    }

    fn navigation_focus_rows(&self) -> Vec<(String, Option<String>)> {
        self.ui
            .tab
            .navigation_history
            .iter()
            .map(|(region, hist)| {
                (
                    region.as_str().to_string(),
                    hist.current_focus().map(|u| u.as_str().to_string()),
                )
            })
            .collect()
    }

    fn current_focus(&self, region: CapRegion) -> Option<EntityUri> {
        ReferenceState::current_focus(self, from_cap_region(region))
            .as_ref()
            .map(cap_id)
    }

    fn focused_cursor(&self, region: CapRegion) -> Option<CapCursor> {
        let r = from_cap_region(region);
        self.ui.tab.focused_cursor.get(&r).map(|cp| CapCursor {
            line: cp.line,
            column: cp.column,
        })
    }

    fn current_focus_region(&self, region: Region) -> Option<EntityUri> {
        self.ui.tab.focused_entity_id.get(&region).map(cap_id)
    }

    fn focused_cursor_region(&self, region: Region) -> Option<CapCursor> {
        self.ui.tab.focused_cursor.get(&region).map(|cp| CapCursor {
            line: cp.line,
            column: cp.column,
        })
    }

    fn has_region_focus(&self, region: Region) -> bool {
        self.ui.tab.focused_entity_id.contains_key(&region)
    }
}

// ─── RefFocusMut ──────────────────────────────────────────────────────

impl RefFocusMut for ReferenceState {
    fn set_focus(&mut self, region: CapRegion, id: EntityUri, cursor: CapCursor) {
        let uri = parse_id_must(&id);
        let r = from_cap_region(region);
        self.ui.tab.focused_entity_id.insert(r, uri.clone());
        self.ui.tab.focused_cursor.insert(
            r,
            CursorPosition {
                line: cursor.line,
                column: cursor.column,
            },
        );
        if r == Region::Main {
            self.ui.tab.focused_block = Some(uri);
        }
    }

    fn clear_focus_if_deleted(&mut self, id: &EntityUri) {
        let uri = parse_id_must(id);
        ReferenceState::clear_focus_if_deleted(self, &uri);
    }

    fn open_active_editor(&mut self, id: EntityUri, content: String, cursor_byte: usize) {
        self.ui.tab.active_editor = Some(super::reference_state::ActiveEditor {
            block_id: id,
            in_memory_content: content,
            cursor_byte,
            dirty: false,
        });
    }

    fn close_active_editor(&mut self) {
        self.ui.tab.active_editor = None;
    }

    fn set_global_focus(&mut self, id: Option<EntityUri>) {
        self.ui.tab.focused_block = id;
    }

    fn set_region_focus(&mut self, region: Region, id: EntityUri, cursor: CapCursor) {
        self.ui.tab.focused_entity_id.insert(region, id);
        self.ui.tab.focused_cursor.insert(
            region,
            CursorPosition {
                line: cursor.line,
                column: cursor.column,
            },
        );
    }

    fn clear_region_focus(&mut self, region: Region) {
        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
    }

    fn reset_focused_cursors_to_start(&mut self) {
        for region in self
            .ui
            .tab
            .focused_entity_id
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            self.ui
                .tab
                .focused_cursor
                .insert(region, CursorPosition::start());
        }
    }

    fn blur_active_editor(&mut self) {
        ReferenceState::blur_active_editor(self);
    }
}

// ─── Phase 6a — RefPeers (read side) ─────────────────────────────────

impl RefPeers for ReferenceState {
    fn peers_len(&self) -> usize {
        self.peers.len()
    }

    fn peer_block_stable_ids(&self, peer_idx: usize) -> Vec<String> {
        self.peers
            .get(peer_idx)
            .map(|p| p.blocks.keys().cloned().collect())
            .unwrap_or_default()
    }

    fn peer_block_content(&self, peer_idx: usize, stable_id: &str) -> Option<String> {
        self.peers
            .get(peer_idx)
            .and_then(|p| p.blocks.get(stable_id))
            .map(|b| b.content.clone())
    }

    fn peer_block_parent(&self, peer_idx: usize, stable_id: &str) -> Option<String> {
        self.peers
            .get(peer_idx)
            .and_then(|p| p.blocks.get(stable_id))
            .and_then(|b| b.parent_stable_id.clone())
    }
}

// ─── Phase 6a — RefPeersMut (write side) ─────────────────────────────

impl RefPeersMut for ReferenceState {
    fn add_peer_from_primary_snapshot(&mut self) -> u64 {
        let peer_id = (self.peers.len() as u64) + 100;
        let peer_blocks: std::collections::HashMap<String, PeerBlock> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                let is_seed = self
                    .domain
                    .block_state
                    .block_documents
                    .get(&b.id)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                !is_seed && !b.is_page()
            })
            .map(|b| {
                let pb = PeerBlock {
                    stable_id: b.id.id().to_string(),
                    parent_stable_id: if b.parent_id.is_no_parent() || b.parent_id.is_sentinel() {
                        None
                    } else {
                        Some(b.parent_id.id().to_string())
                    },
                    content: b.content_text().to_string(),
                };
                (pb.stable_id.clone(), pb)
            })
            .collect();
        let baseline_contents = peer_blocks
            .values()
            .map(|pb| (pb.stable_id.clone(), pb.content.clone()))
            .collect();
        self.peers.push(PeerRefState {
            peer_id,
            blocks: peer_blocks,
            deleted_stable_ids: std::collections::HashSet::new(),
            modified_stable_ids: std::collections::HashSet::new(),
            created_stable_ids: std::collections::HashSet::new(),
            baseline_contents,
        });
        peer_id
    }

    fn peer_apply_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        let peer = &mut self.peers[peer_idx];
        peer.blocks.insert(
            stable_id.to_string(),
            PeerBlock {
                stable_id: stable_id.to_string(),
                parent_stable_id: parent_stable_id.map(|s| s.to_string()),
                content: content.to_string(),
            },
        );
        peer.created_stable_ids.insert(stable_id.to_string());
    }

    fn peer_apply_update(&mut self, peer_idx: usize, stable_id: &str, content: &str) {
        let peer = &mut self.peers[peer_idx];
        let block = peer.blocks.get_mut(stable_id).unwrap_or_else(|| {
            panic!(
                "peer_apply_update: stable_id {stable_id} not in peer {peer_idx} — \
                 generator/precondition bug (a silent no-op here desyncs ref vs SUT)"
            )
        });
        block.content = content.to_string();
        peer.modified_stable_ids.insert(stable_id.to_string());
    }

    fn peer_apply_delete(&mut self, peer_idx: usize, stable_id: &str) {
        let peer = &mut self.peers[peer_idx];
        peer.blocks.remove(stable_id);
        peer.deleted_stable_ids.insert(stable_id.to_string());
    }

    // PeerCharEdit operates at LoroText character level; ref model
    // tracks block-level content only. No-op on the ref side; cross-peer
    // text convergence is checked post-sync by invariants.
    fn peer_apply_char_insert(&mut self, _: usize, _: &str, _: usize, _: &str) {}

    fn peer_apply_char_delete(&mut self, _: usize, _: &str, _: usize, _: usize) {}

    fn peer_sync_from_primary(&mut self, peer_idx: usize) {
        // Bidirectional sync: peer→primary merge, then reflect merged
        // primary state back into the peer's view. Mirrors
        // `sync_with_peer.rs::apply_to_ref` verbatim.
        let modified = self.peers[peer_idx].modified_stable_ids.clone();
        let created = self.peers[peer_idx].created_stable_ids.clone();
        let baseline = self.peers[peer_idx].baseline_contents.clone();
        self.peers[peer_idx].deleted_stable_ids.clear();
        self.peers[peer_idx].modified_stable_ids.clear();
        self.peers[peer_idx].created_stable_ids.clear();
        let peer_blocks: Vec<_> = self.peers[peer_idx].blocks.values().cloned().collect();
        merge_peer_blocks_into_primary(
            &mut self.domain.block_state,
            &peer_blocks,
            &modified,
            &created,
            &baseline,
        );
        self.recanon_and_rebuild();

        // Primary → peer reflect-back: insert any non-seed, non-page
        // primary blocks into the peer (overwrite content so the peer
        // sees post-merge truth).
        let primary_as_peer: Vec<PeerBlock> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                let is_seed = self
                    .domain
                    .block_state
                    .block_documents
                    .get(&b.id)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                !is_seed && !b.is_page()
            })
            .map(|b| PeerBlock {
                stable_id: b.id.id().to_string(),
                parent_stable_id: if b.parent_id.is_no_parent() || b.parent_id.is_sentinel() {
                    None
                } else {
                    Some(b.parent_id.id().to_string())
                },
                content: b.content_text().to_string(),
            })
            .collect();
        let peer = &mut self.peers[peer_idx];
        for pb in primary_as_peer {
            peer.blocks.insert(pb.stable_id.clone(), pb);
        }
        refresh_peer_baseline(peer);
    }

    fn peer_merge_into_primary(&mut self, peer_idx: usize) {
        // Unidirectional: peer→primary merge only, no reflect-back.
        // Mirrors `merge_from_peer.rs::apply_to_ref`.
        let modified = self.peers[peer_idx].modified_stable_ids.clone();
        let created = self.peers[peer_idx].created_stable_ids.clone();
        let baseline = self.peers[peer_idx].baseline_contents.clone();
        self.peers[peer_idx].deleted_stable_ids.clear();
        self.peers[peer_idx].modified_stable_ids.clear();
        self.peers[peer_idx].created_stable_ids.clear();
        let peer_blocks: Vec<_> = self.peers[peer_idx].blocks.values().cloned().collect();
        merge_peer_blocks_into_primary(
            &mut self.domain.block_state,
            &peer_blocks,
            &modified,
            &created,
            &baseline,
        );
        self.recanon_and_rebuild();
        refresh_peer_baseline(&mut self.peers[peer_idx]);
    }
}

// ─── Phase 7 Stage B — extended ref-side cap impls ───────────────────

impl RefFocusRoots for ReferenceState {
    fn expected_focus_root_ids(&self, region: CapRegion) -> BTreeSet<EntityUri> {
        let api_region = from_cap_region(region);
        cap_id_set(self.expected_focus_root_ids(api_region))
    }
}

impl RefLayout for ReferenceState {
    fn layout_block_ids(&self) -> BTreeSet<EntityUri> {
        let ids: BTreeSet<&holon_api::entity_uri::EntityUri> = self
            .domain
            .layout_blocks
            .headline_ids
            .iter()
            .chain(self.domain.layout_blocks.query_source_ids.iter())
            .chain(self.domain.layout_blocks.render_source_ids.iter())
            .collect();
        ids.into_iter().map(cap_id).collect()
    }

    fn profile_block_ids(&self) -> BTreeSet<EntityUri> {
        self.domain.profile_block_ids.iter().map(cap_id).collect()
    }

    fn has_blocks_profile(&self) -> bool {
        self.has_blocks_profile()
    }

    fn all_block_ids(&self) -> BTreeSet<EntityUri> {
        self.domain.block_state.blocks.keys().map(cap_id).collect()
    }

    fn expected_visible_content_ids(&self, region: CapRegion) -> BTreeSet<EntityUri> {
        let focus_roots = self.expected_focus_root_ids(from_cap_region(region));
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                b.content_type != holon_api::ContentType::Source
                    && self.is_descendant_of_any(&b.id, &focus_roots)
            })
            .map(|b| cap_id(&b.id))
            .collect()
    }

    fn has_user_documents(&self) -> bool {
        !self.files.documents.is_empty()
    }

    fn region_entity_focused(&self, region: CapRegion) -> bool {
        self.ui
            .tab
            .focused_entity_id
            .contains_key(&from_cap_region(region))
    }
}

// ─── RefBackend ───────────────────────────────────────────────────────

impl RefBackend for ReferenceState {
    /// Every reference block whose document is NOT a seed document. The runner
    /// has already remapped `id`/`parent_id` into SUT ID space via
    /// `with_resolved_doc_uris`, so these clone directly into the comparison.
    fn non_seed_blocks(&self) -> Vec<holon_api::Block> {
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                let is_seed = self
                    .domain
                    .block_state
                    .block_documents
                    .get(&b.id)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                !is_seed
            })
            .cloned()
            .collect()
    }

    /// Resolved `block_documents` keys whose document is a seed document.
    fn seed_block_ids(&self) -> BTreeSet<EntityUri> {
        self.domain
            .block_state
            .block_documents
            .iter()
            .filter(|(_, doc)| doc.is_no_parent() || doc.is_sentinel())
            .map(|(id, _)| cap_id(id))
            .collect()
    }

    /// Reference blocks as they should appear on disk in org files. The runner
    /// already remapped `id`/`parent_id` into SUT ID space via
    /// `with_resolved_doc_uris` (so `#+ID:`-resolved doc parents are
    /// `block:<uuid>` and split-N placeholders are real UUIDs). The remaining
    /// org-specific step: a
    /// document parent the controller hasn't resolved yet is still a synthetic
    /// doc URI — a key in `self.documents` — and the org parser writes it on
    /// disk as `file:<filename>`. Remap those so the comparison matches.
    fn org_blocks(&self) -> Vec<holon_api::Block> {
        // Blocks always carry a `block:` parent — a top-level org block's
        // parent is its document block (`block:<doc-id>`), which is exactly
        // what `parse_org_file_blocks` reconstructs from the file's `:ID:`
        // drawer. (`EntityUri::file` parents are a future concern, not used
        // for block parentage today.)
        let seed = self.seed_block_ids();
        self.domain
            .block_state
            .blocks
            .values()
            .filter(|b| !seed.contains(&b.id))
            .filter(|b| !b.is_page())
            .cloned()
            .map(|mut b| {
                // On disk the first content line is the headline title, so a
                // trailing `:tag:` group re-parses as org TAGS (the in-memory
                // stores keep the raw content — e.g. after an editor split
                // that lands exactly before a tag group). The disk view must
                // look through that lens.
                crate::pbt::types::apply_org_headline_tag_split(&mut b);
                b
            })
            .collect()
    }
}

impl RefRender for ReferenceState {
    fn current_view(&self) -> String {
        ReferenceState::current_view(self)
    }

    fn active_render_expr_name(&self, region: CapRegion) -> Option<String> {
        let api_region = from_cap_region(region);
        self.active_render_expr_name(api_region)
    }

    fn root_render_expr_name(&self) -> Option<String> {
        // Faithful to inline 10d: read the ROOT render expr (NOT
        // main-panel-preferring) and extract the FunctionCall name.
        // Returns None when there's no root render expr OR when it isn't
        // a FunctionCall; callers disambiguate via has_root_render_expr().
        match self.root_render_expr()? {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    fn has_root_render_expr(&self) -> bool {
        self.root_render_expr().is_some()
    }

    fn root_visible_columns(&self) -> Vec<String> {
        // Faithful to inline 10f: `expected_expr.visible_columns()` on the
        // ROOT render expr. Empty when there's no root render expr.
        self.root_render_expr()
            .map(|e| e.visible_columns())
            .unwrap_or_default()
    }

    fn main_panel_block_id(&self) -> Option<EntityUri> {
        self.main_panel_block_id().as_ref().map(cap_id)
    }

    fn main_panel_render_expr_name(&self) -> Option<String> {
        // The content the main panel should render: its own render expr,
        // falling back to the root render expr (mirrors
        // active_render_expr_name(Main)). Only FunctionCall names are returned.
        match self.main_panel_render_expr().or(self.root_render_expr())? {
            holon_api::render_types::RenderExpr::FunctionCall { name, .. } => Some(name.clone()),
            _ => None,
        }
    }

    fn block_render_mentions(&self, id: &EntityUri, fn_name: &str) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain
            .render_expressions
            .get(&uri)
            .is_some_and(|expr| super::value_fn_invariants::rhai_mentions(expr, fn_name))
    }

    fn has_block_render_expr(&self, id: &EntityUri) -> bool {
        self.domain.render_expressions.contains_key(id)
    }
}

impl RefWatches for ReferenceState {
    fn active_watch_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.mcp.active_watches.keys().cloned().collect();
        ids.sort();
        ids
    }

    /// Evaluate the watch query against the (already SUT-ID-space-resolved)
    /// block state and stringify each `Value` into the `WatchRow` shape.
    /// NULL/non-string values become `None`, exactly as `Value::as_string()`
    /// returns `None`.
    fn expected_watch_rows(&self, query_id: &str) -> Vec<WatchRow> {
        let Some(watch_spec) = self.mcp.active_watches.get(query_id) else {
            return Vec::new();
        };
        self.query_results(watch_spec)
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|(k, v)| (k, v.as_string().map(str::to_string)))
                    .collect()
            })
            .collect()
    }

    fn watch_query_columns(&self, query_id: &str) -> Vec<String> {
        self.mcp
            .active_watches
            .get(query_id)
            .map(|ws| ws.query.columns.clone())
            .unwrap_or_default()
    }

    fn watch_block_raw_sql(&self, query_id: &str) -> String {
        self.mcp
            .active_watches
            .get(query_id)
            .map(|ws| ws.query.to_block_raw_sql())
            .unwrap_or_default()
    }
}

impl RefGlobalFocus for ReferenceState {
    fn global_focused_block(&self) -> Option<EntityUri> {
        self.ui.tab.focused_block.as_ref().map(cap_id)
    }
}

impl RefTaskState for ReferenceState {
    fn task_state_of(&self, id: &EntityUri) -> Option<String> {
        let uri = parse_id(id)?;
        let block = self.domain.block_state.blocks.get(&uri)?;
        block
            .properties
            .get("task_state")
            .and_then(|v| v.as_string().map(str::to_owned))
    }
}

// ─── Documents (block↔doc + document registry) ───────────────────────

impl RefDocuments for ReferenceState {
    fn document_of(&self, id: &EntityUri) -> Option<EntityUri> {
        self.domain.block_state.block_documents.get(id).cloned()
    }

    fn blocks_in_document(&self, doc: &EntityUri) -> Vec<EntityUri> {
        self.domain
            .block_state
            .block_documents
            .iter()
            .filter(|(_, uri)| *uri == doc)
            .map(|(id, _)| id.clone())
            .collect()
    }

    fn document_uris(&self) -> Vec<EntityUri> {
        self.files.documents.keys().cloned().collect()
    }

    fn document_filename(&self, uri: &EntityUri) -> Option<String> {
        self.files.documents.get(uri).cloned()
    }

    fn doc_uri_by_name(&self, name: &str) -> Option<EntityUri> {
        ReferenceState::doc_uri_by_name(self, name)
    }

    fn document_count(&self) -> usize {
        self.files.documents.len()
    }
}

impl RefDocumentsMut for ReferenceState {
    fn mint_document_uri(&mut self) -> EntityUri {
        self.next_synthetic_doc_uri()
    }

    fn create_document(&mut self, file_name: String) -> EntityUri {
        self.create_document_ref(file_name)
    }
}

// ─── Navigation history + pins ───────────────────────────────────────

impl RefNavHistory for ReferenceState {
    fn can_go_back(&self, region: Region) -> bool {
        ReferenceState::can_go_back(self, region)
    }

    fn can_go_forward(&self, region: Region) -> bool {
        ReferenceState::can_go_forward(self, region)
    }

    fn is_unpinnable(&self, history_id: i64) -> bool {
        self.ui.user.open_pins.iter().any(|(region, pins)| {
            let focus = self.current_focus(*region);
            pins.iter()
                .any(|p| p.history_id == history_id && p.block_id.is_some() && p.block_id != focus)
        })
    }
}

impl RefNavHistoryMut for ReferenceState {
    fn nav_focus_push(&mut self, region: Region, block_id: Option<EntityUri>) {
        let history = self.ui.tab.navigation_history.entry(region).or_default();
        history.entries.truncate(history.cursor + 1);
        history.entries.push(block_id.clone());
        history.cursor = history.entries.len() - 1;

        // Close all open rows in the region, then insert a new open row.
        // `next_history_id` mirrors SQLite AUTOINCREMENT (monotonic across
        // INSERTs); `next_pin_ts` is the logical pin timestamp. Both are MODEL
        // state observed by the focus/nav invariants.
        let history_id = self.ui.tab.next_history_id;
        self.ui.tab.next_history_id += 1;
        let added_ts_logical = self.ui.user.next_pin_ts;
        self.ui.user.next_pin_ts += 1;
        let pins = self.ui.user.open_pins.entry(region).or_default();
        pins.clear();
        pins.push(super::reference_state::OpenPinEntry {
            history_id,
            block_id,
            added_ts_logical,
        });
    }

    fn nav_history_back(&mut self, region: Region) {
        if let Some(history) = self.ui.tab.navigation_history.get_mut(&region)
            && history.cursor > 0
        {
            history.cursor -= 1;
        }
    }

    fn nav_history_forward(&mut self, region: Region) {
        if let Some(history) = self.ui.tab.navigation_history.get_mut(&region)
            && history.cursor < history.entries.len() - 1
        {
            history.cursor += 1;
        }
    }

    fn add_pin(&mut self, region: Region, block_id: EntityUri) {
        // Move-to-top dedup (`provider.rs::focus_pin`): bump an existing open
        // pin's timestamp, else INSERT (minting a history id).
        let added_ts_logical = self.ui.user.next_pin_ts;
        self.ui.user.next_pin_ts += 1;

        let pins = self.ui.user.open_pins.entry(region).or_default();
        if let Some(existing) = pins
            .iter_mut()
            .find(|p| p.block_id.as_ref() == Some(&block_id))
        {
            existing.added_ts_logical = added_ts_logical;
        } else {
            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            self.ui.user.open_pins.entry(region).or_default().push(
                super::reference_state::OpenPinEntry {
                    history_id,
                    block_id: Some(block_id),
                    added_ts_logical,
                },
            );
        }
    }

    fn remove_pin(&mut self, history_id: i64) {
        for pins in self.ui.user.open_pins.values_mut() {
            pins.retain(|p| p.history_id != history_id);
        }
    }

    fn mark_navigation_visit(&mut self, block_id: &EntityUri) -> bool {
        let first_visit = self.ui.tab.seen_focus_targets.insert(block_id.clone());
        self.ui.tab.last_navigate_first_visit = first_visit;
        first_visit
    }
}

// ─── Toggles + drawers ───────────────────────────────────────────────

impl RefToggles for ReferenceState {
    fn is_expanded(&self, id: &EntityUri) -> bool {
        self.ui.tab.expanded_toggles.contains(id)
    }

    fn is_drawer_open(&self, block_id: &str) -> bool {
        // Default open: production default layout boots both sidebars open.
        self.ui
            .tab
            .drawer_open
            .get(block_id)
            .copied()
            .unwrap_or(true)
    }
}

impl RefTogglesMut for ReferenceState {
    fn set_expanded(&mut self, id: &EntityUri, expanded: bool) {
        if expanded {
            self.ui.tab.expanded_toggles.insert(id.clone());
        } else {
            self.ui.tab.expanded_toggles.remove(id);
        }
    }

    fn set_drawer_open(&mut self, block_id: &str, open: bool) {
        self.ui.tab.drawer_open.insert(block_id.to_string(), open);
    }
}

// ─── Undo/redo history stacks ────────────────────────────────────────

impl RefHistory for ReferenceState {
    fn has_undo(&self) -> bool {
        !self.action.undo_stack.is_empty()
    }

    fn has_redo(&self) -> bool {
        !self.action.redo_stack.is_empty()
    }
}

impl RefHistoryMut for ReferenceState {
    fn undo(&mut self) {
        ReferenceState::pop_undo_to_redo(self);
    }

    fn redo(&mut self) {
        ReferenceState::pop_redo_to_undo(self);
    }
}

// ─── Render mutation ─────────────────────────────────────────────────

impl RefRenderMut for ReferenceState {
    fn set_current_view(&mut self, name: String) {
        self.ui.user.current_view = name;
    }
}

// ─── Local Ref caps (crate-typed) ────────────────────────────────────
//
// These traits name crate-local (`MutationEvent`, `TestQuery`, `WatchSpec`,
// `TodoKeywordSet`) or `holon_frontend` (`CollectionNavigator`) types, so they
// cannot live in `holon-pbt-core` (which depends only on `holon-api`). They are
// the reference-side siblings of the pbt-core `Ref*` caps for the transitions
// whose data crosses that dependency boundary. Not `#[capmap_adapter]`
// (transition-consumed, not invariant-selected), matching the pbt-core
// `Ref*Mut` convention.

/// Compound reference-model block/document writes that carry crate-local
/// mutation/generator types. One honest compound per transition (the raw
/// block/document maps stay encapsulated on `ReferenceState`).
pub trait RefMutation {
    /// Apply a `MutationEvent` via the plain path (ToggleState / TriggerSlash).
    fn apply_mutation_event(&mut self, event: &super::types::MutationEvent);

    /// Apply a NON-peer `MutationEvent` via the richer ApplyMutation path
    /// (undo snapshot + `block_documents` + render-expr + focus follow-up).
    fn apply_external_mutation(&mut self, event: &super::types::MutationEvent);

    /// Bulk-insert externally-authored blocks into a document (BulkExternalAdd).
    fn add_external_blocks(&mut self, blocks: &[holon_api::block::Block], doc_uri: &EntityUri);

    /// Re-materialize an org document into the reference model (WriteOrgFile).
    fn write_org_document(
        &mut self,
        filename: &str,
        blocks: &[holon_api::block::Block],
        keyword_set: Option<&super::generators::TodoKeywordSet>,
    );
}

impl RefMutation for ReferenceState {
    fn apply_mutation_event(&mut self, event: &super::types::MutationEvent) {
        ReferenceState::apply_mutation(self, event);
    }

    fn apply_external_mutation(&mut self, event: &super::types::MutationEvent) {
        ReferenceState::apply_external_mutation(self, event);
    }

    fn add_external_blocks(&mut self, blocks: &[holon_api::block::Block], doc_uri: &EntityUri) {
        ReferenceState::add_external_blocks(self, blocks, doc_uri);
    }

    fn write_org_document(
        &mut self,
        filename: &str,
        blocks: &[holon_api::block::Block],
        keyword_set: Option<&super::generators::TodoKeywordSet>,
    ) {
        ReferenceState::write_org_document(self, filename, blocks, keyword_set);
    }
}

/// Model-computed navigation / render predicates — heavy shadow-interpreter and
/// navigator computations only the wide-PBT reference can honestly answer (pure
/// slices have no render interpreter). `build_reference_navigator` returns a
/// `holon_frontend` navigator, which forces this trait crate-local.
pub trait RefModelPredict {
    /// Whether a click on `uri` in `region` dispatches `navigation.focus`.
    fn predicts_navigation_focus(&self, uri: &EntityUri, region: Region) -> bool;

    /// Block ids in the predicted LeftSidebar render set.
    fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri>;

    /// Whether the user has written an index.org with query+render blocks.
    fn has_user_index_org(&self) -> bool;

    /// Whether the active main-panel layout renders `id` as a `draggable`.
    fn block_renders_draggable(&self, id: &EntityUri) -> bool;

    /// Block ids the active main-panel layout renders (its query's rendered set).
    fn main_rendered_block_ids(&self) -> BTreeSet<EntityUri>;

    /// A reference-state navigator mirroring production's arrow-key handler for
    /// `region`. Owned (`Box`) so the caller holds no borrow across the
    /// navigation loop's block mutations.
    fn build_reference_navigator(
        &self,
        region: Region,
    ) -> Option<Box<dyn holon_frontend::navigation::CollectionNavigator>>;
}

impl RefModelPredict for ReferenceState {
    fn predicts_navigation_focus(&self, uri: &EntityUri, region: Region) -> bool {
        ReferenceState::predicts_navigation_focus(self, uri, region)
    }

    fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri> {
        ReferenceState::predicted_sidebar_navigation_targets(self)
    }

    fn has_user_index_org(&self) -> bool {
        ReferenceState::has_user_index_org(self)
    }

    fn block_renders_draggable(&self, id: &EntityUri) -> bool {
        ReferenceState::block_renders_draggable(self, id)
    }

    fn main_rendered_block_ids(&self) -> BTreeSet<EntityUri> {
        ReferenceState::main_rendered_block_ids(self)
    }

    fn build_reference_navigator(
        &self,
        region: Region,
    ) -> Option<Box<dyn holon_frontend::navigation::CollectionNavigator>> {
        ReferenceState::build_reference_navigator(self, region)
    }
}

/// Write-side watch registration carrying the crate-local `TestQuery`. Read side
/// is the pbt-core [`RefWatches`].
pub trait RefWatchesMut {
    /// Register (or replace) a watch by id with its compiled query + language
    /// (SetupWatch).
    fn register_watch(
        &mut self,
        query_id: String,
        query: super::query::TestQuery,
        language: holon_api::QueryLanguage,
    );

    /// Remove a previously-registered watch by id (RemoveWatch).
    fn remove_watch(&mut self, query_id: &str);
}

impl RefWatchesMut for ReferenceState {
    fn register_watch(
        &mut self,
        query_id: String,
        query: super::query::TestQuery,
        language: holon_api::QueryLanguage,
    ) {
        self.mcp
            .active_watches
            .insert(query_id, super::query::WatchSpec { query, language });
    }

    fn remove_watch(&mut self, query_id: &str) {
        self.mcp.active_watches.remove(query_id);
    }
}

// ─── CapProvider — the keystone (ADR 0007 / PbtCompositionDesign §6) ───
//
// Lets a live `ReferenceState` BE the ref `CapMap` that `run_selected`
// consumes, so any slice (and the generic subsystem-shrink PBT) can read the
// single real oracle instead of a bespoke parallel ref model.
//
// We register the read caps the composed catalog consumes today
// (`RefBackend` + `RefBlockTree` for the block invariants, `RefEditorMirror`
// for the editor invariants, `RefLayout` for the windowed
// `inv-frontend-bounds-rendered`). Further widening of the read surface
// (focus/render/…) stays deferred (it could newly *select* catalog invariants —
// the "catalog scope creep" risk in the plan) until each is wired.
//
// `RefEditorMirror` is registered unconditionally: selection is an AND over
// the SUT and ref cap sets (`Needs::selected_against`), so the editor
// invariants still deselect for a config whose SUT has no editor — the ref
// carrying the cap is harmless.
impl holon_pbt_core::composition::CapProvider for ReferenceState {
    fn register(self: Arc<Self>, caps: &mut holon_pbt_core::composition::CapMap) {
        caps.insert(self.clone() as Arc<dyn RefBackend>);
        caps.insert(self.clone() as Arc<dyn RefBlockTree>);
        caps.insert(self.clone() as Arc<dyn RefEditorMirror>);
        // `RefLayout` carries the layout-block / document metadata the windowed
        // `inv-frontend-bounds-rendered` reads (`has_user_documents`,
        // `region_entity_focused`). Registering it unconditionally is harmless to
        // existing slices: selection ANDs the SUT and ref cap sets, and only the
        // windowed slice supplies the matching `SutLayout + SutViewModel`.
        caps.insert(self.clone() as Arc<dyn RefLayout>);
        // `RefWatches` carries the active-watch query set + expected rows the B5
        // watch invariants read (E1 SutWatchRows relocation). Harmless to existing
        // slices: only the frontend slice supplies the matching `SutWatchRows`.
        caps.insert(self.clone() as Arc<dyn RefWatches>);
        // `RefFocus` carries the per-region navigation focus + expected focus roots
        // the `inv-navigation-focus` / `inv-focus-roots` invariants read (SutHandle
        // decomposition: NavigateFocus onto SutFocusWrite). Harmless to existing
        // slices: selection ANDs the SUT and ref cap sets, and only a slice that
        // also supplies `SutSqlProjection` (+`SutBackend`) selects the focus
        // invariants — and only the navigation slice drives real focus data.
        caps.insert(self.clone() as Arc<dyn RefFocus>);
        // `RefRender` carries the active-view / render-expr metadata the ViewModel
        // invariants read (`inv-view-selection`, the C3 renderer cluster). The
        // logic already lives on `ReferenceState`; this just exposes it on the ref
        // `CapMap`. Harmless to existing slices: selection ANDs the SUT and ref cap
        // sets, and only a slice supplying `SutViewModel`/`SutRenderer` selects it.
        caps.insert(self.clone() as Arc<dyn RefRender>);
        // `RefTaskState` + `RefGlobalFocus` carry the task-state / global-focus
        // metadata the `value_fn_provider_*` ViewModel invariants read (C3 batch 2).
        // Logic already on `ReferenceState`; harmless to existing slices (selection
        // ANDs SUT∧ref cap sets — only a `SutViewModel` slice selects them).
        caps.insert(self.clone() as Arc<dyn RefTaskState>);
        caps.insert(self as Arc<dyn RefGlobalFocus>);
    }
}

/// Build the ref `CapMap` from a [`Resolved`] [`ReferenceState`] — the keystone
/// helper the slices and the generic PBT use in place of `ref_map`/`full_ref_map`.
///
/// Requires the [`Resolved`] witness: the comparison caps built here compare ids
/// directly against the SUT, so the ref's ids must already live in the SUT's id
/// space (see [`ReferenceState::with_resolved_doc_uris`] /
/// [`Resolved::identity`]). An unresolved ref is a compile error here.
pub fn reference_state_ref_caps(
    state: Resolved<Arc<ReferenceState>>,
) -> holon_pbt_core::composition::CapMap {
    let mut caps = holon_pbt_core::composition::CapMap::new();
    holon_pbt_core::composition::CapProvider::register(state.into_inner(), &mut caps);
    caps
}

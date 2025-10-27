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
//! - Read methods returning ids: produce `String` via
//!   `EntityUri::as_str().to_string()`.
//! - Read methods taking ids: parse via `EntityUri::from_str` and propagate
//!   `None` on failure (a malformed id can't match anything in the block tree).
//! - Write methods taking ids: parse + `.expect()` — wide PBT only ever sees
//!   valid `EntityUri`s, so a parse failure is a programmer error.

use std::collections::BTreeSet;
use std::sync::Arc;

use holon_api::Region;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::CapCursor;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefPeers;
use holon_pbt_core::capabilities::RefPeersMut;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::RefWatch;
use holon_pbt_core::capabilities::WatchRow;

use super::peer_ops::PeerBlock;
use super::reference_state::CursorPosition;
use super::reference_state::PeerRefState;
use super::reference_state::ReferenceState;
use super::reference_state::Resolved;
use super::state_machine::merge_peer_blocks_into_primary;

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
        // E-solid shadow mesh: lazily seed at the first AddPeer, mirror any
        // pending primary state, pad to the SUT's fed height (the height the
        // SUT's snapshot export sees), THEN fork — so the shadow peer's base
        // clock matches the SUT peer's (clock_parity_spike s3/s7: staggered
        // fork heights are exactly what an unpadded shadow gets wrong).
        if self.shadow_mesh.is_none() {
            self.shadow_mesh = Some(super::shadow_mesh::ShadowMesh::seeded_from_blocks(
                &self.domain.block_state.blocks,
            ));
        }
        let mesh = self.shadow_mesh.as_mut().expect("just seeded");
        mesh.catch_up_primary(&self.domain.block_state.blocks);
        if let Some(h) = *self.clock_feed.lock().expect("clock_feed lock") {
            mesh.pad_primary_to(h);
        }
        let shadow_peer_id = mesh.fork_peer();

        let peer_id = (self.peers.len() as u64) + 100;
        assert_eq!(
            shadow_peer_id, peer_id,
            "shadow peer id must match the ref/SUT peer id scheme (100 + idx)"
        );
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
        self.peers.push(PeerRefState {
            peer_id,
            blocks: peer_blocks,
            deleted_stable_ids: std::collections::HashSet::new(),
            modified_stable_ids: std::collections::HashSet::new(),
            created_stable_ids: std::collections::HashSet::new(),
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
        self.shadow_mesh
            .as_ref()
            .expect("shadow mesh exists once peers do")
            .peer_create(peer_idx, parent_stable_id, content, stable_id);
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
        self.shadow_mesh
            .as_ref()
            .expect("shadow mesh exists once peers do")
            .peer_update(peer_idx, stable_id, content);
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
        self.shadow_mesh
            .as_ref()
            .expect("shadow mesh exists once peers do")
            .peer_delete(peer_idx, stable_id);
        let peer = &mut self.peers[peer_idx];
        peer.blocks.remove(stable_id);
        peer.deleted_stable_ids.insert(stable_id.to_string());
    }

    // PeerCharEdit: mirror into the shadow peer's LoroText, then read the
    // peer's block-level content back from the shadow — the shadow mesh
    // closes the former "ref tracks block-level content only" gap for free.
    fn peer_apply_char_insert(&mut self, peer_idx: usize, stable_id: &str, pos: usize, text: &str) {
        let mesh = self
            .shadow_mesh
            .as_ref()
            .expect("shadow mesh exists once peers do");
        mesh.peer_char_insert(peer_idx, stable_id, pos, text);
        let content = mesh
            .peer_content(peer_idx, stable_id)
            .unwrap_or_else(|| panic!("shadow peer {peer_idx} lacks {stable_id}"));
        let peer = &mut self.peers[peer_idx];
        peer.blocks
            .get_mut(stable_id)
            .unwrap_or_else(|| panic!("peer_apply_char_insert: {stable_id} not in peer {peer_idx}"))
            .content = content;
        peer.modified_stable_ids.insert(stable_id.to_string());
    }

    fn peer_apply_char_delete(&mut self, peer_idx: usize, stable_id: &str, pos: usize, len: usize) {
        let mesh = self
            .shadow_mesh
            .as_ref()
            .expect("shadow mesh exists once peers do");
        mesh.peer_char_delete(peer_idx, stable_id, pos, len);
        let content = mesh
            .peer_content(peer_idx, stable_id)
            .unwrap_or_else(|| panic!("shadow peer {peer_idx} lacks {stable_id}"));
        let peer = &mut self.peers[peer_idx];
        peer.blocks
            .get_mut(stable_id)
            .unwrap_or_else(|| panic!("peer_apply_char_delete: {stable_id} not in peer {peer_idx}"))
            .content = content;
        peer.modified_stable_ids.insert(stable_id.to_string());
    }

    fn peer_sync_from_primary(&mut self, peer_idx: usize) {
        // Bidirectional sync: peer→primary merge, then reflect merged
        // primary state back into the peer's view. Mirrors
        // `sync_with_peer.rs::apply_to_ref` verbatim.
        //
        // Shadow first: mirror pending primary state, pad to the SUT's fed
        // height (the pre-sync boundary), run the REAL CRDT sync on the
        // shadow docs — the merge below then CONSUMES the shadow's converged
        // text + tie-break order instead of modeling them.
        {
            let mesh = self
                .shadow_mesh
                .as_ref()
                .expect("shadow mesh exists once peers do");
            mesh.catch_up_primary(&self.domain.block_state.blocks);
            if let Some(h) = *self.clock_feed.lock().expect("clock_feed lock") {
                mesh.pad_primary_to(h);
            }
            mesh.sync_peer_bidirectional(peer_idx);
        }
        let modified = self.peers[peer_idx].modified_stable_ids.clone();
        let created = self.peers[peer_idx].created_stable_ids.clone();
        self.peers[peer_idx].deleted_stable_ids.clear();
        self.peers[peer_idx].modified_stable_ids.clear();
        self.peers[peer_idx].created_stable_ids.clear();
        let peer_blocks: Vec<_> = self.peers[peer_idx].blocks.values().cloned().collect();
        merge_peer_blocks_into_primary(
            &mut self.domain.block_state,
            &peer_blocks,
            &modified,
            &created,
            self.shadow_mesh.as_ref().expect("shadow mesh present"),
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
    }

    fn peer_merge_into_primary(&mut self, peer_idx: usize) {
        // Unidirectional: peer→primary merge only, no reflect-back.
        // Mirrors `merge_from_peer.rs::apply_to_ref`.
        {
            let mesh = self
                .shadow_mesh
                .as_ref()
                .expect("shadow mesh exists once peers do");
            mesh.catch_up_primary(&self.domain.block_state.blocks);
            if let Some(h) = *self.clock_feed.lock().expect("clock_feed lock") {
                mesh.pad_primary_to(h);
            }
            mesh.merge_peer_into_primary(peer_idx);
        }
        let modified = self.peers[peer_idx].modified_stable_ids.clone();
        let created = self.peers[peer_idx].created_stable_ids.clone();
        self.peers[peer_idx].deleted_stable_ids.clear();
        self.peers[peer_idx].modified_stable_ids.clear();
        self.peers[peer_idx].created_stable_ids.clear();
        let peer_blocks: Vec<_> = self.peers[peer_idx].blocks.values().cloned().collect();
        merge_peer_blocks_into_primary(
            &mut self.domain.block_state,
            &peer_blocks,
            &modified,
            &created,
            self.shadow_mesh.as_ref().expect("shadow mesh present"),
        );
        self.recanon_and_rebuild();
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

impl RefViewSelection for ReferenceState {
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
}

impl RefWatch for ReferenceState {
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
        // windowed slice supplies the matching `SutLayout + SutViewSelection`.
        caps.insert(self.clone() as Arc<dyn RefLayout>);
        // `RefWatch` carries the active-watch query set + expected rows the B5
        // watch invariants read (E1 SutWatch relocation). Harmless to existing
        // slices: only the frontend slice supplies the matching `SutWatch`.
        caps.insert(self.clone() as Arc<dyn RefWatch>);
        // `RefFocus` carries the per-region navigation focus + expected focus roots
        // the `inv-navigation-focus` / `inv-focus-roots` invariants read (SutHandle
        // decomposition: NavigateFocus onto SutFocusWrite). Harmless to existing
        // slices: selection ANDs the SUT and ref cap sets, and only a slice that
        // also supplies `SutSqlProjection` (+`SutBackend`) selects the focus
        // invariants — and only the navigation slice drives real focus data.
        caps.insert(self.clone() as Arc<dyn RefFocus>);
        // `RefViewSelection` carries the active-view / render-expr metadata the
        // ViewModel invariants read (`inv-view-selection`, the C3 renderer
        // cluster). The logic already lives on `ReferenceState`; this just
        // exposes it on the ref `CapMap`. Harmless to existing slices:
        // selection ANDs the SUT and ref cap sets, and only a slice supplying
        // `SutViewSelection`/`SutRenderer` selects it.
        caps.insert(self.clone() as Arc<dyn RefViewSelection>);
        // `RefTaskState` + `RefGlobalFocus` carry the task-state / global-focus
        // metadata the `value_fn_provider_*` ViewModel invariants read (C3 batch 2).
        // Logic already on `ReferenceState`; harmless to existing slices (selection
        // ANDs SUT∧ref cap sets — only a `SutViewSelection` slice selects them).
        caps.insert(self.clone() as Arc<dyn RefTaskState>);
        caps.insert(self as Arc<dyn RefGlobalFocus>);
    }
}

/// Build the ref `CapMap` from a [`Resolved`] [`ReferenceState`] — the keystone
/// helper the slices and the generic PBT use in place of
/// `ref_map`/`full_ref_map`.
///
/// Requires the [`Resolved`] witness: the comparison caps built here compare
/// ids directly against the SUT, so the ref's ids must already live in the
/// SUT's id space (see [`ReferenceState::with_resolved_doc_uris`] /
/// [`Resolved::identity`]). An unresolved ref is a compile error here.
pub fn reference_state_ref_caps(
    state: Resolved<Arc<ReferenceState>>,
) -> holon_pbt_core::composition::CapMap {
    let mut caps = holon_pbt_core::composition::CapMap::new();
    holon_pbt_core::composition::CapProvider::register(state.into_inner(), &mut caps);
    caps
}

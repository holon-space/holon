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

use holon_api::Region;
use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::{
    CapCursor, CapRegion, RefBlockTree, RefBlockTreeMut, RefEditorMirror, RefEditorMirrorMut,
    RefFocus, RefFocusMut, RefFocusRoots, RefGlobalFocus, RefLayout, RefLifecycle, RefPeers,
    RefPeersMut, RefRender, RefTaskState, RefWatches,
};

use super::peer_ops::PeerBlock;
use super::reference_state::PeerRefState;
use super::state_machine::{merge_peer_blocks_into_primary, refresh_peer_baseline};

use super::reference_state::{CursorPosition, ReferenceState};

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
        self.app_started
    }
    fn is_properly_setup(&self) -> bool {
        self.is_properly_setup()
    }
    fn enable_loro(&self) -> bool {
        self.variant.enable_loro
    }
    fn last_transition_kind(&self) -> Option<&'static str> {
        self.last_transition_kind
    }
    fn atomic_editor_enabled() -> bool {
        ReferenceState::atomic_editor_enabled()
    }
}

// ─── RefBlockTree ─────────────────────────────────────────────────────

impl RefBlockTree for ReferenceState {
    fn block_content(&self, id: &EntityUri) -> Option<&str> {
        let uri = parse_id(id)?;
        self.block_state
            .blocks
            .get(&uri)
            .map(|b| b.content.as_str())
    }

    fn is_text_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.block_state
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
        let b = self.block_state.blocks.get(&uri)?;
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
        self.layout_blocks.contains(&uri)
    }

    fn is_focusable(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.layout_blocks.is_focusable(&uri)
    }

    fn is_no_content_update(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.layout_blocks.render_source_ids.contains(&uri)
            || self.layout_blocks.query_source_ids.contains(&uri)
            || self.profile_block_ids.contains(&uri)
    }

    fn is_page_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.block_state
            .blocks
            .get(&uri)
            .is_some_and(|b| b.is_page())
    }

    fn all_non_seed_block_ids(&self) -> BTreeSet<EntityUri> {
        self.block_state
            .blocks
            .keys()
            .filter(|uri| {
                let is_seed = self
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
        if let Some(b) = self.block_state.blocks.get_mut(&uri) {
            b.content = text.to_string();
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
        self.active_editor.as_ref().map(|e| cap_id(&e.block_id))
    }

    fn active_editor_text(&self) -> Option<&str> {
        self.active_editor
            .as_ref()
            .map(|e| e.in_memory_content.as_str())
    }

    fn active_editor_cursor(&self) -> Option<usize> {
        self.active_editor.as_ref().map(|e| e.cursor_byte)
    }
}

// ─── RefEditorMirrorMut ───────────────────────────────────────────────

impl RefEditorMirrorMut for ReferenceState {
    fn type_chars(&mut self, text: &str) {
        if let Some(editor) = self.active_editor.as_mut() {
            editor.type_chars(text);
        }
    }

    fn delete_backward(&mut self, count: usize) {
        if let Some(editor) = self.active_editor.as_mut() {
            editor.delete_backward(count);
        }
    }

    fn move_cursor(&mut self, byte_position: usize) {
        if let Some(editor) = self.active_editor.as_mut() {
            editor.move_cursor(byte_position);
        }
    }
}

// ─── RefFocus ─────────────────────────────────────────────────────────

impl RefFocus for ReferenceState {
    fn current_focus(&self, region: CapRegion) -> Option<EntityUri> {
        ReferenceState::current_focus(self, from_cap_region(region))
            .as_ref()
            .map(cap_id)
    }

    fn focused_cursor(&self, region: CapRegion) -> Option<CapCursor> {
        let r = from_cap_region(region);
        self.focused_cursor.get(&r).map(|cp| CapCursor {
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
        self.focused_entity_id.insert(r, uri.clone());
        self.focused_cursor.insert(
            r,
            CursorPosition {
                line: cursor.line,
                column: cursor.column,
            },
        );
        if r == Region::Main {
            self.focused_block = Some(uri);
        }
    }

    fn clear_focus_if_deleted(&mut self, id: &EntityUri) {
        let uri = parse_id_must(id);
        ReferenceState::clear_focus_if_deleted(self, &uri);
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
//
// AddPeer is fully migrated. The remaining 5 Loro transitions
// (PeerEdit{Create,Update,Delete}, PeerCharEdit, SyncWithPeer,
// MergeFromPeer) panic with `unimplemented!` — wide PBT keeps using
// their original `apply_to_ref` path until a slice consumer drives
// the migration. This mirrors the Phase 6 "trait surface first,
// blanket impls on demand" stance.

impl RefPeersMut for ReferenceState {
    fn add_peer_from_primary_snapshot(&mut self) -> u64 {
        let peer_id = (self.peers.len() as u64) + 100;
        let peer_blocks: std::collections::HashMap<String, PeerBlock> = self
            .block_state
            .blocks
            .values()
            .filter(|b| {
                let is_seed = self
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
        if let Some(block) = peer.blocks.get_mut(stable_id) {
            block.content = content.to_string();
            peer.modified_stable_ids.insert(stable_id.to_string());
        }
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
            &mut self.block_state,
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
            .block_state
            .blocks
            .values()
            .filter(|b| {
                let is_seed = self
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
            &mut self.block_state,
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
            .layout_blocks
            .headline_ids
            .iter()
            .chain(self.layout_blocks.query_source_ids.iter())
            .chain(self.layout_blocks.render_source_ids.iter())
            .collect();
        ids.into_iter().map(cap_id).collect()
    }

    fn profile_block_ids(&self) -> BTreeSet<EntityUri> {
        self.profile_block_ids.iter().map(cap_id).collect()
    }

    fn has_blocks_profile(&self) -> bool {
        self.has_blocks_profile()
    }
}

impl RefRender for ReferenceState {
    fn active_render_expr_name(&self, region: CapRegion) -> Option<String> {
        let api_region = from_cap_region(region);
        self.active_render_expr_name(api_region)
    }

    fn has_root_render_expr(&self) -> bool {
        self.root_render_expr().is_some()
    }
}

impl RefWatches for ReferenceState {
    fn active_watch_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.active_watches.keys().cloned().collect();
        ids.sort();
        ids
    }
}

impl RefGlobalFocus for ReferenceState {
    fn global_focused_block(&self) -> Option<EntityUri> {
        self.focused_block.as_ref().map(cap_id)
    }
}

impl RefTaskState for ReferenceState {
    fn task_state_of(&self, id: &EntityUri) -> Option<String> {
        let uri = parse_id(id)?;
        let block = self.block_state.blocks.get(&uri)?;
        block
            .properties
            .get("task_state")
            .and_then(|v| v.as_string().map(str::to_owned))
    }
}

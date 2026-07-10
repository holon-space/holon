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

use holon_api::ContentType;
use holon_api::EdgeFieldUpdate;
use holon_api::Region;
use holon_api::SourceLanguage;
use holon_api::block::Block;
use holon_api::entity_uri::EntityUri;
use holon_orgmode::OrgBlockExt;
use holon_orgmode::OrgDocumentExt;
use holon_pbt_core::capabilities::AdviceExpectation;
use holon_pbt_core::capabilities::CapCursor;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefAdvice;
use holon_pbt_core::capabilities::RefApplyMutationMut;
use holon_pbt_core::capabilities::RefBackend;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefBoot;
use holon_pbt_core::capabilities::RefBootMut;
use holon_pbt_core::capabilities::RefClock;
use holon_pbt_core::capabilities::RefClockMut;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefDocumentsMut;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefNavHistory;
use holon_pbt_core::capabilities::RefNavHistoryMut;
use holon_pbt_core::capabilities::RefPeers;
use holon_pbt_core::capabilities::RefPeersMut;
use holon_pbt_core::capabilities::RefPins;
use holon_pbt_core::capabilities::RefPinsMut;
use holon_pbt_core::capabilities::RefRenderExpr;
use holon_pbt_core::capabilities::RefSqlCardinality;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::RefToggle;
use holon_pbt_core::capabilities::RefToggleMut;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::capabilities::RefViewSelectionMut;
use holon_pbt_core::capabilities::RefWatch;
use holon_pbt_core::capabilities::RefWatchesMut;
use holon_pbt_core::capabilities::RefWiring;
use holon_pbt_core::capabilities::WatchRow;

use super::advice_expectation::active_rule;
use super::advice_expectation::expectation_for;
use super::advice_expectation::matview_rows_for;
use super::peer_ops::PeerBlock;
use super::reference_state::CursorPosition;
use super::reference_state::OpenPinEntry;
use super::reference_state::PeerRefState;
use super::reference_state::ReferenceState;
use super::reference_state::Resolved;
use super::state_machine::merge_peer_blocks_into_primary;
use crate::pbt::types::MutationApply;

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
    fn next_doc_id(&self) -> usize {
        self.action.next_doc_id
    }
    fn next_block_id(&self) -> usize {
        self.domain.block_state.next_id
    }
    fn has_undo_history(&self) -> bool {
        !self.action.undo_stack.is_empty()
    }
    fn has_redo_history(&self) -> bool {
        !self.action.redo_stack.is_empty()
    }
}

// ─── RefClock (ADR 0024 §6 AdvanceDay) ────────────────────────────────

impl RefClock for ReferenceState {
    fn today(&self) -> String {
        self.clock.today.clone()
    }
    fn expected_journal_day_count(&self) -> usize {
        self.clock.visited_days.len()
    }
}

impl RefClockMut for ReferenceState {
    fn advance_day(&mut self, days: i64) {
        self.clock.advance_day(days);
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

    fn is_source_block(&self, id: &EntityUri) -> bool {
        let Some(uri) = parse_id(id) else {
            return false;
        };
        self.domain
            .block_state
            .blocks
            .get(&uri)
            .is_some_and(|b| b.content_type == ContentType::Source)
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
            // paths now share one normalization. Marks are re-derived from
            // the committed text (replacing any previous mark set) — the org
            // writeback→re-ingest fixed point the SUT converges to.
            let (content, marks) =
                super::types::normalize_content_for_org_roundtrip(text, b.content_type);
            b.content = content;
            b.marks = marks;
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

    fn undo_last_and_reset_cursors(&mut self) {
        self.pop_undo_to_redo();
        // Undo may restore different content — reset all cursors.
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
                .insert(region, super::reference_state::CursorPosition::start());
        }
    }

    fn redo_last_and_reset_cursors(&mut self) {
        self.pop_redo_to_undo();
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
                .insert(region, super::reference_state::CursorPosition::start());
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

    fn peer_modified_stable_ids(&self, peer_idx: usize) -> Vec<String> {
        self.peers
            .get(peer_idx)
            .map(|p| p.modified_stable_ids.iter().cloned().collect())
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

    fn has_user_index_org(&self) -> bool {
        ReferenceState::has_user_index_org(self)
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

// ─── RefPins / RefPinsMut ─────────────────────────────────────────────

impl RefPins for ReferenceState {
    fn open_pin_history_ids(&self) -> Vec<i64> {
        self.ui
            .user
            .open_pins
            .values()
            .flatten()
            .map(|p| p.history_id)
            .collect()
    }

    fn is_closeable_pin(&self, history_id: i64) -> bool {
        self.ui.user.open_pins.iter().any(|(region, pins)| {
            let focus = self.current_focus(*region);
            pins.iter()
                .any(|p| p.history_id == history_id && p.block_id.is_some() && p.block_id != focus)
        })
    }
}

impl RefPinsMut for ReferenceState {
    fn close_pin(&mut self, history_id: i64) {
        for pins in self.ui.user.open_pins.values_mut() {
            pins.retain(|p| p.history_id != history_id);
        }
    }

    fn upsert_open_pin(&mut self, region: Region, block_id: &EntityUri) {
        // Move-to-top dedup, mirroring `provider.rs::focus_pin`: SELECT existing
        // open `(region, block_id)`; UPDATE timestamp if found, else INSERT.
        // Bumping `next_pin_ts` (not `next_history_id`) on the UPDATE path
        // matches the no-INSERT path of `update_pin_timestamp.sql`.
        let added_ts_logical = self.ui.user.next_pin_ts;
        self.ui.user.next_pin_ts += 1;

        let pins = self.ui.user.open_pins.entry(region).or_default();
        if let Some(existing) = pins
            .iter_mut()
            .find(|p| p.block_id.as_ref() == Some(block_id))
        {
            existing.added_ts_logical = added_ts_logical;
        } else {
            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            self.ui
                .user
                .open_pins
                .entry(region)
                .or_default()
                .push(OpenPinEntry {
                    history_id,
                    block_id: Some(block_id.clone()),
                    added_ts_logical,
                });
        }
    }
}

// ─── RefNavHistory / RefNavHistoryMut ─────────────────────────────────

impl RefNavHistory for ReferenceState {
    fn can_go_back(&self, region: Region) -> bool {
        ReferenceState::can_go_back(self, region)
    }
    fn can_go_forward(&self, region: Region) -> bool {
        ReferenceState::can_go_forward(self, region)
    }
    fn predicts_navigation_focus(&self, block_id: &EntityUri, region: Region) -> bool {
        ReferenceState::predicts_navigation_focus(self, block_id, region)
    }
    fn predicted_sidebar_navigation_targets(&self) -> Vec<EntityUri> {
        ReferenceState::predicted_sidebar_navigation_targets(self)
    }
    fn drawer_is_open(&self, panel_id: &str) -> bool {
        holon_layout_testing::LayoutRefState::drawer_is_open(self, panel_id)
    }
}

impl RefNavHistoryMut for ReferenceState {
    fn nav_step_back(&mut self, region: Region) {
        if let Some(history) = self.ui.tab.navigation_history.get_mut(&region)
            && history.cursor > 0
        {
            history.cursor -= 1;
        }
        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
        // Blur on nav: clears `active_editor`, committing pending text only
        // under a real editor (mirrors prod's `on_blur`).
        self.blur_active_editor();
    }

    fn nav_step_forward(&mut self, region: Region) {
        if let Some(history) = self.ui.tab.navigation_history.get_mut(&region)
            && history.cursor < history.entries.len() - 1
        {
            history.cursor += 1;
        }
        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
        self.blur_active_editor();
    }

    fn nav_go_home(&mut self, region: Region) {
        // Idempotent like same-target focus: when already home (current focus is
        // `None`), prod's `focus(region, None)` writes NO new `navigation_history`
        // / `open_pins` row. Pushing a duplicate would let `NavigateBack` walk back
        // through phantom home entries the SUT never created.
        let already_home = self.current_focus(region).is_none();
        if !already_home {
            let history = self.ui.tab.navigation_history.entry(region).or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(None);
            history.cursor = history.entries.len() - 1;

            // `go_home` = `focus(region, None)`: close all open rows in region,
            // then insert a NULL-block_id home row. Kept in `open_pins` so
            // `next_history_id` aligns with SQLite's AUTOINCREMENT; filtered out of
            // `expected_focus_root_ids` (None block_id).
            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            let added_ts_logical = self.ui.user.next_pin_ts;
            self.ui.user.next_pin_ts += 1;
            let pins = self.ui.user.open_pins.entry(region).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: None,
                added_ts_logical,
            });
        }

        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
        // Mirror prod: `maybe_mirror_navigation_focus` clears the global
        // `focused_block` on go_home regardless of which region triggered it.
        self.ui.tab.focused_block = None;
        self.blur_active_editor();
    }

    fn nav_focus(&mut self, region: Region, block_id: &EntityUri) {
        // Re-focusing the region's current target is idempotent in prod:
        // `navigation.focus` on the active target writes no new history row.
        let already_focused = self.current_focus(region).as_ref() == Some(block_id);

        // Budget model: the first navigation to a root creates its watch matviews;
        // recorded pre-insert because the budget invariant only sees post-apply.
        self.ui.tab.last_navigate_first_visit =
            self.ui.tab.seen_focus_targets.insert(block_id.clone());

        if !already_focused {
            let history = self.ui.tab.navigation_history.entry(region).or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(block_id.clone()));
            history.cursor = history.entries.len() - 1;

            // Mirror provider.rs `focus`: close all open rows in the region, then
            // insert a new open row. `next_history_id` follows AUTOINCREMENT (INSERT
            // only); a same-block re-focus inserts no row, so the counter holds.
            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            let added_ts_logical = self.ui.user.next_pin_ts;
            self.ui.user.next_pin_ts += 1;
            let pins = self.ui.user.open_pins.entry(region).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: Some(block_id.clone()),
                added_ts_logical,
            });
        }

        self.ui.tab.focused_entity_id.remove(&region);
        self.ui.tab.focused_cursor.remove(&region);
        // Mirror `UiState::set_focus`: the nav target becomes the global focus.
        self.ui.tab.focused_block = Some(block_id.clone());
        self.blur_active_editor();
    }
}

// ─── RefDocuments / RefDocumentsMut ───────────────────────────────────

impl RefDocuments for ReferenceState {
    fn document_names(&self) -> Vec<String> {
        self.files.documents.values().cloned().collect()
    }
    fn has_document(&self, file_name: &str) -> bool {
        self.files
            .documents
            .values()
            .any(|name| name.as_str() == file_name)
    }
    fn document_count(&self) -> usize {
        self.files.documents.len()
    }
    fn doc_uri_by_name(&self, name: &str) -> Option<EntityUri> {
        ReferenceState::doc_uri_by_name(self, name)
    }
    fn block_document_of(&self, block_id: &EntityUri) -> Option<EntityUri> {
        self.domain
            .block_state
            .block_documents
            .get(block_id)
            .cloned()
    }
    fn has_non_seed_advice_rule(&self) -> bool {
        !crate::pbt::advice_expectation::non_seed_advice_rule_blocks(&self.domain.block_state)
            .is_empty()
    }
    fn document_uris(&self) -> Vec<EntityUri> {
        self.files.documents.keys().cloned().collect()
    }
    fn has_document_uri(&self, uri: &EntityUri) -> bool {
        self.files.documents.contains_key(uri)
    }
}

impl RefDocumentsMut for ReferenceState {
    fn insert_document(&mut self, file_name: &str) {
        let doc_uri = self.next_synthetic_doc_uri();
        self.files
            .documents
            .insert(doc_uri.clone(), file_name.to_string());

        let doc_name = std::path::Path::new(file_name)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(file_name)
            .to_string();
        let mut doc_block = Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name);
        doc_block.set_page(true);
        // New empty documents don't have #+TODO: headers — keywords only appear
        // after the file is written with content.
        self.domain
            .block_state
            .blocks
            .insert(doc_uri.clone(), doc_block);
        self.domain
            .block_state
            .block_documents
            .insert(doc_uri.clone(), doc_uri);
    }

    fn remove_document(&mut self, file_name: &str) {
        let doc_uri = self
            .files
            .documents
            .iter()
            .find(|(_, name)| name.as_str() == file_name)
            .map(|(uri, _)| uri.clone())
            .unwrap_or_else(|| {
                panic!(
                    "RefDocumentsMut::remove_document: '{file_name}' not in files.documents \
                     (precondition hole)"
                )
            });
        self.files.documents.remove(&doc_uri);

        // Cascade-delete the page block + all descendants through the same
        // `Mutation::Delete` machinery `ApplyMutation` uses (BFS over parent_id),
        // then re-canonicalize exactly like apply_mutation does.
        let mutation = holon_pbt_core::types::Mutation::Delete {
            id: doc_uri.clone(),
        };
        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        mutation.apply_to(&mut blocks);
        crate::org_utils::assign_reference_sequences_canonical(&mut blocks);
        let surviving: std::collections::BTreeMap<EntityUri, Block> =
            blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.domain
            .block_state
            .block_documents
            .retain(|id, _| surviving.contains_key(id));
        self.domain.block_state.blocks = surviving;
        self.rebuild_profile_tracking();

        self.clear_focus_if_deleted(&doc_uri);
    }

    fn seed_org_file(
        &mut self,
        filename: &str,
        blocks: &[Block],
        todo_keywords: Option<Vec<holon_api::TaskState>>,
    ) {
        let doc_name = std::path::Path::new(filename)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(filename)
            .to_string();
        let doc_uri = self
            .doc_uri_by_name(&doc_name)
            .unwrap_or_else(|| self.next_synthetic_doc_uri());
        self.files
            .documents
            .insert(doc_uri.clone(), filename.to_string());

        // Remove old content blocks from this document (re-writing the same file).
        let old_block_ids: Vec<EntityUri> = self
            .domain
            .block_state
            .block_documents
            .iter()
            .filter(|(_, uri)| **uri == doc_uri)
            .map(|(id, _)| id.clone())
            .collect();
        for id in &old_block_ids {
            self.domain.block_state.blocks.remove(id);
            self.domain.block_state.block_documents.remove(id);
            self.domain.layout_blocks.remove(id);
            self.domain.render_expressions.remove(id);
        }

        // Add the page block (tags ⊇ ["Page"]) for this org file.
        let mut doc_block =
            Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name.clone());
        doc_block.set_page(true);
        // Mirror the SUT parser: a `#+TODO:` header lands on the document block as
        // the `todo_keywords` property.
        if let Some(kw) = &todo_keywords {
            doc_block.set_todo_keywords(Some(kw.clone()));
        }
        self.domain
            .block_state
            .blocks
            .insert(doc_uri.clone(), doc_block);
        self.domain
            .block_state
            .block_documents
            .insert(doc_uri.clone(), doc_uri.clone());

        // Insert the generated blocks directly — no re-parsing. Top-level headings
        // parented to `GEN_PLACEHOLDER` are remapped to the resolved doc uri; the
        // `ID` renderer hint is stripped; layout classification is derived from each
        // block's `source_language`, mirroring the org parser's index.org handling.
        let placeholder =
            EntityUri::block(crate::pbt::transitions::write_org_file::GEN_PLACEHOLDER);
        let is_index = filename == "index.org";
        for (seq, generated) in blocks.iter().enumerate() {
            let mut block = generated.clone();
            if block.parent_id == placeholder {
                block.parent_id = doc_uri.clone();
            }
            block.properties.remove("ID");
            // File-parse order: the org parser splits trailing `:tag:` groups
            // off the RAW headline line first, then extracts inline marks from
            // the tag-less title — mirror that order or mark offsets computed
            // over a still-tagged line diverge from the SUT's.
            crate::pbt::types::apply_org_headline_tag_split(&mut block);
            let (content, marks) = crate::pbt::types::normalize_content_for_org_roundtrip(
                &block.content,
                block.content_type,
            );
            block.content = content;
            block.marks = marks;
            block.set_sequence(seq as i64);
            let block_uri = block.id.clone();

            if is_index
                && block.content_type == ContentType::Source
                && let Some(sl) = block.source_language.as_ref()
            {
                if sl.as_query().is_some() {
                    self.domain
                        .layout_blocks
                        .headline_ids
                        .insert(block.parent_id.clone());
                    self.domain
                        .layout_blocks
                        .query_source_ids
                        .insert(block_uri.clone());
                } else if matches!(sl, SourceLanguage::Render) {
                    self.domain
                        .layout_blocks
                        .headline_ids
                        .insert(block.parent_id.clone());
                    self.domain
                        .layout_blocks
                        .render_source_ids
                        .insert(block_uri.clone());
                    if let Ok(expr) = self.interpreter.parse_dsl(block.content.as_str()) {
                        self.domain
                            .render_expressions
                            .insert(block_uri.clone(), expr);
                    }
                }
            }

            self.domain
                .block_state
                .block_documents
                .insert(block_uri.clone(), doc_uri.clone());
            self.domain.block_state.blocks.insert(block_uri, block);
        }

        // Re-assign sequences using canonical ordering.
        let mut all_blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        crate::org_utils::assign_reference_sequences_canonical(&mut all_blocks);
        self.domain.block_state.blocks =
            all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();

        self.rebuild_profile_tracking();
        self.pre_startup_file_count += 1;
    }
}

// ─── RefToggle / RefToggleMut ─────────────────────────────────────────

impl RefToggle for ReferenceState {
    fn is_expanded(&self, id: &EntityUri) -> bool {
        self.ui.tab.expanded_toggles.contains(id)
    }
}

impl RefToggleMut for ReferenceState {
    fn set_expanded(&mut self, id: &EntityUri, expanded: bool) {
        if expanded {
            self.ui.tab.expanded_toggles.insert(id.clone());
        } else {
            self.ui.tab.expanded_toggles.remove(id);
        }
    }
    fn toggle_drawer(&mut self, id: &str) {
        // Default-open, so an untracked drawer flips to closed.
        let current = holon_layout_testing::LayoutRefState::drawer_is_open(self, id);
        self.ui.tab.drawer_open.insert(id.to_string(), !current);
    }
}

// ─── RefRenderExpr ────────────────────────────────────────────────────

impl RefRenderExpr for ReferenceState {
    fn render_expr_ids(&self) -> Vec<EntityUri> {
        self.domain.render_expressions.keys().cloned().collect()
    }
    fn has_render_expr(&self, id: &EntityUri) -> bool {
        self.domain.render_expressions.contains_key(id)
    }
    fn render_expr_mentions(&self, id: &EntityUri, needle: &str) -> bool {
        self.domain
            .render_expressions
            .get(id)
            .is_some_and(|expr| crate::pbt::value_fn_invariants::rhai_mentions(expr, needle))
    }
}

// ─── RefViewSelectionMut ──────────────────────────────────────────────

impl RefViewSelectionMut for ReferenceState {
    fn set_current_view(&mut self, view: &str) {
        self.ui.user.current_view = view.to_string();
    }
}

// ─── RefWatchesMut ────────────────────────────────────────────────────

impl RefWatchesMut for ReferenceState {
    type WatchSpec = crate::pbt::query::WatchSpec;
    fn insert_watch(&mut self, query_id: &str, spec: Self::WatchSpec) {
        self.mcp.active_watches.insert(query_id.to_string(), spec);
    }
    fn remove_watch(&mut self, query_id: &str) {
        self.mcp.active_watches.remove(query_id);
    }
}

// ─── RefBoot / RefBootMut ─────────────────────────────────────────────

impl RefBoot for ReferenceState {
    fn pre_startup_directory_count(&self) -> usize {
        self.pre_startup_directories.len()
    }
    fn pre_startup_file_count(&self) -> usize {
        self.pre_startup_file_count
    }
    fn git_initialized(&self) -> bool {
        self.git_initialized
    }
    fn jj_initialized(&self) -> bool {
        self.jj_initialized
    }
    fn root_layout_block_id(&self) -> Option<EntityUri> {
        ReferenceState::root_layout_block_id(self)
    }
}

impl RefBootMut for ReferenceState {
    fn push_pre_startup_directory(&mut self, path: &str) {
        self.pre_startup_directories.push(path.to_string());
    }
    fn mark_git_initialized(&mut self) {
        self.git_initialized = true;
    }
    fn mark_jj_initialized(&mut self) {
        self.jj_initialized = true;
        self.git_initialized = true; // jj git init also creates .git
    }
    fn boot_app(&mut self) {
        use crate::pbt::transitions::start_app::SEEDED_SIDEBAR_WATCH_ID;
        use crate::pbt::transitions::start_app::load_seed_profile_into_ref;
        use crate::pbt::transitions::start_app::seed_booted_layout_into_ref;
        use crate::pbt::transitions::start_app::seeded_sidebar_watch_spec;

        self.action.app_started = true;

        // Freshness mirrors prod `seed_default_layout`: the default layout is only
        // seeded when `block:root-layout` is absent at boot. A pre-startup user
        // `index.org` keeps the well-known root id, so it suppresses the default seed.
        let fresh = !self
            .domain
            .block_state
            .blocks
            .contains_key(&holon_api::root_layout_block_uri());

        // Default layout boots both sidebars as open drawers.
        if fresh {
            self.ui
                .tab
                .drawer_open
                .insert("block:default-left-sidebar".to_string(), true);
            self.ui
                .tab
                .drawer_open
                .insert("block:default-right-sidebar".to_string(), true);
        }

        seed_booted_layout_into_ref(self, fresh);

        // Register the production seeded left-sidebar watch on the ref side.
        self.mcp.active_watches.insert(
            SEEDED_SIDEBAR_WATCH_ID.to_string(),
            seeded_sidebar_watch_spec(),
        );

        load_seed_profile_into_ref(self);

        // FU-10 mirror: prod `seed_default_layout` calls `navigation::focus(Main,
        // block:journals)` on fresh DBs ONLY, inserting a navigation_history row.
        if fresh {
            let journals_uri = EntityUri::block("journals");
            let history = self
                .ui
                .tab
                .navigation_history
                .entry(Region::Main)
                .or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(journals_uri.clone()));
            history.cursor = history.entries.len() - 1;

            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            let added_ts_logical = self.ui.user.next_pin_ts;
            self.ui.user.next_pin_ts += 1;
            let pins = self.ui.user.open_pins.entry(Region::Main).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: Some(journals_uri),
                added_ts_logical,
            });
        }
    }
}

// ─── RefWiring / RefLayoutInteract / RefLayoutMutate ──────────────────

impl RefWiring for ReferenceState {
    fn has_cap_set(&self) -> bool {
        self.cap_set.is_some()
    }

    fn caps_available(&self, caps: &[holon_pbt_core::composition::CapId]) -> bool {
        ReferenceState::caps_available(self, caps)
    }
}

impl RefLayoutInteract for ReferenceState {
    fn render_source_ids(&self) -> BTreeSet<EntityUri> {
        self.domain
            .layout_blocks
            .render_source_ids
            .iter()
            .cloned()
            .collect()
    }
    fn query_source_ids(&self) -> BTreeSet<EntityUri> {
        self.domain
            .layout_blocks
            .query_source_ids
            .iter()
            .cloned()
            .collect()
    }
    fn is_immutable(&self, id: &EntityUri) -> bool {
        self.domain.layout_blocks.is_immutable(id)
    }
    fn block_renders_draggable(&self, id: &EntityUri) -> bool {
        ReferenceState::block_renders_draggable(self, id)
    }
    fn main_rendered_block_ids(&self) -> BTreeSet<EntityUri> {
        ReferenceState::main_rendered_block_ids(self)
    }
    fn region_focused_entity(&self, region: CapRegion) -> Option<EntityUri> {
        self.focused_entity(from_cap_region(region)).cloned()
    }
    fn focused_main_editable(&self) -> Option<EntityUri> {
        ReferenceState::focused_main_editable(self)
    }
    fn block_has_tag(&self, id: &EntityUri, tag: &str) -> bool {
        self.domain
            .block_state
            .blocks
            .get(id)
            .is_some_and(|b| b.tags.contains(tag))
    }
    fn doc_has_editable_text(&self, doc_uri: &EntityUri) -> bool {
        self.domain.block_state.blocks.values().any(|b| {
            b.parent_id == *doc_uri
                && b.content_type == ContentType::Text
                && !b.is_page()
                && !self.domain.layout_blocks.contains(&b.id)
        })
    }

    fn headline_ids(&self) -> Vec<EntityUri> {
        self.domain
            .layout_blocks
            .headline_ids
            .iter()
            .cloned()
            .collect()
    }
}

impl RefApplyMutationMut for ReferenceState {
    fn apply_content_mutation(&mut self, mutation: &holon_pbt_core::types::Mutation) {
        use holon_pbt_core::types::Mutation;

        if let Mutation::Create { id, parent_id, .. } = mutation {
            let doc_uri = if parent_id.is_no_parent() || parent_id.is_sentinel() {
                parent_id.clone()
            } else {
                // The new block belongs to its parent's document. But when the
                // parent is itself a top-level page (its own `block_documents`
                // entry is `no_parent`/`sentinel`), the page IS the document —
                // the child lives in the page's org file, not in the page's
                // (sentinel) document. Inheriting the sentinel would misclassify
                // the child as a seed block and drop it from the `/org` view.
                match self.domain.block_state.block_documents.get(parent_id) {
                    Some(doc) if !doc.is_no_parent() && !doc.is_sentinel() => doc.clone(),
                    _ => parent_id.clone(),
                }
            };
            self.domain
                .block_state
                .block_documents
                .insert(id.clone(), doc_uri);
        }

        let mut blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        mutation.apply_to(&mut blocks);
        crate::org_utils::assign_reference_sequences_canonical(&mut blocks);
        self.domain.block_state.blocks = blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.rebuild_profile_tracking();

        if let Mutation::Update { id, fields, .. } = mutation
            && self.domain.layout_blocks.render_source_ids.contains(id)
            && fields.contains_key("content")
            && let Some(block) = self.domain.block_state.blocks.get(id)
            && let Some(expr) =
                super::reference_state::render_expr_from_rhai(block.content.as_str())
        {
            self.domain.render_expressions.insert(id.clone(), expr);
        }

        self.domain.block_state.next_id += 1;

        if let Mutation::Update { id, fields, .. } = mutation
            && fields.contains_key("content")
        {
            self.reset_cursor_if_focused(id);
        }
    }
}

impl RefLayoutMutate for ReferenceState {
    fn apply_click_focus(&mut self, region: Region, block_id: &EntityUri) {
        use crate::pbt::reference_state::CursorPosition;
        use crate::pbt::reference_state::OpenPinEntry;
        // A real click outside the active editor blurs it (real-editor-only commit
        // via `blur_active_editor`). Same-block clicks don't blur.
        if self
            .ui
            .tab
            .active_editor
            .as_ref()
            .is_some_and(|e| e.block_id != *block_id)
        {
            self.blur_active_editor();
        }
        if self.predicts_navigation_focus(block_id, region) {
            // Sidebar `selectable` → `navigation.focus(region=main)`: mirror the
            // nav-history push (see navigate_focus.rs for rationale).
            let history = self
                .ui
                .tab
                .navigation_history
                .entry(Region::Main)
                .or_default();
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(block_id.clone()));
            history.cursor = history.entries.len() - 1;

            let history_id = self.ui.tab.next_history_id;
            self.ui.tab.next_history_id += 1;
            let added_ts_logical = self.ui.user.next_pin_ts;
            self.ui.user.next_pin_ts += 1;
            let pins = self.ui.user.open_pins.entry(Region::Main).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: Some(block_id.clone()),
                added_ts_logical,
            });

            self.ui.tab.focused_entity_id.remove(&Region::Main);
            self.ui.tab.focused_cursor.remove(&Region::Main);
            self.ui.tab.focused_block = Some(block_id.clone());
        } else {
            // Editor focus only — the nav cursor is unchanged (ADR 0010: focus is
            // in-memory state, set directly).
            self.ui.tab.focused_block = Some(block_id.clone());
            self.ui
                .tab
                .focused_entity_id
                .insert(region, block_id.clone());
            self.ui
                .tab
                .focused_cursor
                .insert(region, CursorPosition::start());
        }
    }

    fn apply_slash_delete(&mut self, block_id: &EntityUri) {
        use holon_pbt_core::types::Mutation;
        use holon_pbt_core::types::MutationEvent;
        use holon_pbt_core::types::MutationSource;
        self.push_undo_snapshot();
        self.apply_mutation(&MutationEvent {
            source: MutationSource::UI,
            mutation: Mutation::Delete {
                id: block_id.clone(),
            },
        });
        self.clear_focus_if_deleted(block_id);
    }

    fn set_edge_field_value(&mut self, id: &EntityUri, update: &EdgeFieldUpdate) {
        let block = self
            .domain
            .block_state
            .blocks
            .get_mut(id)
            .expect("set_edge_field_value: subject block must exist (precondition)");
        // Direct field assignment (public edge-field columns); `is_page` is
        // computed from `tags` on read, so no cached state to sync.
        match update {
            EdgeFieldUpdate::Tags(tags) => block.tags = tags.clone(),
            EdgeFieldUpdate::Requires(reqs) => block.requires = reqs.clone(),
            EdgeFieldUpdate::AdviceSuppressed(reqs) => block.advice_suppressed = reqs.clone(),
        }
    }

    fn bulk_add_blocks(&mut self, doc_uri: &EntityUri, blocks: &[Block]) {
        for block in blocks {
            let mut block = block.clone();
            // Mirror the org round-trip normalization `Mutation::apply_to` does.
            // Parse order: tag split off the raw headline first, THEN mark
            // extraction (see write_org_file ingest above).
            crate::pbt::types::apply_org_headline_tag_split(&mut block);
            let (content, marks) = crate::pbt::types::normalize_content_for_org_roundtrip(
                &block.content,
                block.content_type,
            );
            block.content = content;
            block.marks = marks;
            let id = block.id.clone();
            self.domain.block_state.blocks.insert(id.clone(), block);
            self.domain
                .block_state
                .block_documents
                .insert(id, doc_uri.clone());
        }
        let mut all_blocks: Vec<Block> = self.domain.block_state.blocks.values().cloned().collect();
        crate::org_utils::assign_reference_sequences_canonical(&mut all_blocks);
        self.domain.block_state.blocks =
            all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        self.rebuild_profile_tracking();
        self.domain.block_state.next_id += blocks.len();
    }

    fn create_block_under(&mut self, parent: &EntityUri, content: &str) {
        ReferenceState::create_block_under(self, parent, content);
    }

    fn create_block_under_with_id(&mut self, parent: &EntityUri, content: &str, id: EntityUri) {
        ReferenceState::create_block_under_with_id(self, parent, content, id);
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

impl RefSqlCardinality for ReferenceState {
    fn block_count(&self) -> usize {
        self.domain.block_state.blocks.len()
    }
    fn document_count(&self) -> usize {
        self.files.documents.len()
    }
    fn active_watch_count(&self) -> usize {
        self.mcp.active_watches.len()
    }
    fn last_navigate_first_visit(&self) -> bool {
        self.ui.tab.last_navigate_first_visit
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

impl holon_pbt_core::capabilities::RefArrowNav for ReferenceState {
    type Direction = holon_frontend::navigation::NavDirection;

    fn region_has_focus(&self, region: Region) -> bool {
        self.ui.tab.focused_entity_id.contains_key(&region)
    }

    fn apply_arrow_navigate(
        &mut self,
        region: Region,
        direction: holon_frontend::navigation::NavDirection,
        steps: u8,
    ) {
        use holon_frontend::navigation::Boundary;
        use holon_frontend::navigation::CursorHint;
        use holon_frontend::navigation::NavDirection;

        use super::reference_state::CursorPosition;

        let mut current_id = self
            .ui
            .tab
            .focused_entity_id
            .get(&region)
            .expect("ArrowNavigate requires focused entity")
            .clone();
        let mut cursor = self
            .ui
            .tab
            .focused_cursor
            .get(&region)
            .copied()
            .unwrap_or(CursorPosition::start());

        let navigator = self.build_reference_navigator(region);

        for _ in 0..steps {
            let content = self
                .domain
                .block_state
                .blocks
                .get(&current_id)
                .map(|b| b.content.as_str())
                .unwrap_or("");
            let line_count = if content.is_empty() {
                1
            } else {
                content.split('\n').count()
            };
            let last_line = line_count.saturating_sub(1);

            let crosses_block = match direction {
                NavDirection::Up => cursor.line == 0,
                NavDirection::Down => cursor.line >= last_line,
                NavDirection::Left => cursor.line == 0 && cursor.column == 0,
                NavDirection::Right => {
                    let line_len = content
                        .split('\n')
                        .nth(cursor.line)
                        .map(|l| l.len())
                        .unwrap_or(0);
                    cursor.line >= last_line && cursor.column >= line_len
                }
            };

            if crosses_block {
                if let Some(ref nav) = navigator {
                    let boundary = match direction {
                        NavDirection::Up => Boundary::Top,
                        NavDirection::Down => Boundary::Bottom,
                        NavDirection::Left => Boundary::Left,
                        NavDirection::Right => Boundary::Right,
                    };
                    let hint = CursorHint {
                        column: cursor.column,
                        boundary,
                    };
                    if let Some(target) = nav.navigate(&current_id, direction, &hint) {
                        current_id = target.block_id.clone();
                        let target_content = self
                            .domain
                            .block_state
                            .blocks
                            .get(&current_id)
                            .map(|b| b.content.as_str())
                            .unwrap_or("");
                        let offset = holon_frontend::navigation::placement_to_offset(
                            target_content,
                            target.placement,
                        );
                        let (line, col) =
                            holon_frontend::navigation::offset_to_line_col(target_content, offset);
                        cursor = CursorPosition { line, column: col };
                    }
                }
            } else {
                match direction {
                    NavDirection::Up => {
                        cursor.line = cursor.line.saturating_sub(1);
                    }
                    NavDirection::Down => {
                        cursor.line = (cursor.line + 1).min(last_line);
                    }
                    NavDirection::Left => {
                        if cursor.column > 0 {
                            cursor.column -= 1;
                        } else if cursor.line > 0 {
                            cursor.line -= 1;
                            let prev_line_len = content
                                .split('\n')
                                .nth(cursor.line)
                                .map(|l| l.len())
                                .unwrap_or(0);
                            cursor.column = prev_line_len;
                        }
                    }
                    NavDirection::Right => {
                        let line_len = content
                            .split('\n')
                            .nth(cursor.line)
                            .map(|l| l.len())
                            .unwrap_or(0);
                        if cursor.column < line_len {
                            cursor.column += 1;
                        } else if cursor.line < last_line {
                            cursor.line += 1;
                            cursor.column = 0;
                        }
                    }
                }
            }
        }

        self.ui.tab.focused_block = Some(current_id.clone());
        self.ui.tab.focused_entity_id.insert(region, current_id);
        self.ui.tab.focused_cursor.insert(region, cursor);
    }
}

impl holon_pbt_core::capabilities::RefTaskStateToggle for ReferenceState {
    fn rendered_state_toggle_ids(&self) -> Vec<EntityUri> {
        let owned_render_expr = self
            .main_panel_render_expr()
            .or_else(|| self.root_render_expr())
            .cloned()
            .unwrap_or_else(super::reference_state::default_root_render_expr);

        let main_focus_roots = self.expected_focus_root_ids(holon_api::Region::Main);
        let visible_text_block_ids: Vec<EntityUri> = self
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| {
                b.content_type == holon_api::ContentType::Text
                    && !b.is_page()
                    && !self.domain.layout_blocks.contains(&b.id)
                    && self.is_descendant_of_any(&b.id, &main_focus_roots)
            })
            .map(|b| b.id.clone())
            .collect();

        let rows: Vec<holon_api::widget_spec::DataRow> = visible_text_block_ids
            .iter()
            .filter_map(|id| self.domain.block_state.blocks.get(id))
            .map(super::reference_state::block_to_data_row)
            .collect();
        let arc_rows: Vec<Arc<_>> = rows.into_iter().map(Arc::new).collect();
        // ALLOW(pbt-sut-handle-frontend-simulation): generator-side render lookup
        let vm = holon_frontend::interpret_pure(&owned_render_expr, &arc_rows, self);
        vm.snapshot()
            .state_toggle_block_ids()
            .into_iter()
            .filter_map(|id| holon_api::EntityUri::parse(&id).ok())
            .collect()
    }

    fn apply_toggle_state(
        &mut self,
        block_id: &EntityUri,
        new_state: holon_pbt_core::types::CycleTarget,
    ) {
        self.push_undo_snapshot();
        self.apply_mutation(&holon_pbt_core::types::MutationEvent {
            source: holon_pbt_core::types::MutationSource::UI,
            mutation: holon_pbt_core::types::Mutation::Update {
                id: block_id.clone(),
                fields: [(
                    "task_state".to_string(),
                    holon_api::Value::String(new_state.keyword().to_string()),
                )]
                .into(),
            },
        });
    }
}

/// Advice-weave read surface (ADR 0021/0022) — delegates to the pure
/// `advice_expectation` module over the resolved block map. Plain reads
/// suffice: the `ReferenceState` behind the caps is already `Resolved`
/// (`with_resolved_doc_uris` → `remapped_doc_uris`), so `block.id` and the
/// `advice_suppressed` edge targets are already in SUT id space; no per-method
/// remapping is needed here. Ids are rendered via `EntityUri::as_str()` — the
/// scheme-form `block_raw.id` carries — so anchor/candidate strings compare
/// directly against the SUT advice matview.
impl RefAdvice for ReferenceState {
    fn advice_expectation(&self, anchor: &str) -> AdviceExpectation {
        let blocks = &self.domain.block_state.blocks;
        let Some(rule) = active_rule(blocks) else {
            return AdviceExpectation::default();
        };
        let anchor_id =
            EntityUri::parse(anchor).expect("advice anchor id must be a valid EntityUri");
        expectation_for(blocks, &rule, &anchor_id)
    }

    fn advice_matview_rows(&self) -> Vec<(String, String, u32)> {
        let blocks = &self.domain.block_state.blocks;
        let Some(rule) = active_rule(blocks) else {
            return Vec::new();
        };
        matview_rows_for(blocks, &rule)
            .into_iter()
            .map(|(a, c, n)| (a.as_str().to_string(), c.as_str().to_string(), n))
            .collect()
    }

    fn advice_matview_name(&self) -> Option<String> {
        active_rule(&self.domain.block_state.blocks).map(|rule| rule.name.matview_name())
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
        // `RefAdvice` carries the advice-weave expectation (ADR 0021/0022) the
        // `advice rows woven` keystone invariant reads. Harmless to existing
        // slices: selection ANDs SUT∧ref cap sets, and only a slice supplying the
        // matching SUT advice-matview cap selects it.
        caps.insert(self.clone() as Arc<dyn RefAdvice>);
        // `RefClock` carries the calendar-day model + predicted journal count the
        // `AdvanceDay` journal-count invariant reads (ADR 0024 §6). Harmless to
        // existing slices: selection ANDs SUT∧ref cap sets, and only a slice
        // supplying the matching `SutJournalCount` selects it.
        caps.insert(self.clone() as Arc<dyn RefClock>);
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

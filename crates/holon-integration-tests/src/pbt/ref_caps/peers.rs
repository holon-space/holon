//! `RefPeers` / `RefPeersMut` (Phase 6a) — thin orphan-rule impls.
//!
//! The trait lives in `holon-pbt-core`, the concrete `ReferenceState` in this
//! crate, so the impls must stay here (orphan rule). The Loro-private *logic*
//! moved to `holon_loro_testing::ref_ext::LoroRefExt` (RefStateSplit Inc 5);
//! these impls delegate into `self.loro`, handing it primary block data through
//! `holon_api`-typed parameters and a layout predicate. The peer→primary merge
//! (`merge_peer_blocks_into_primary`) and the `recanon_and_rebuild` fixpoint
//! stay core-side here — "the module hands intent; the core computes the merge"
//! (`PbtCompositionDesign.md` §5.5).

use holon_pbt_core::capabilities::RefPeers;
use holon_pbt_core::capabilities::RefPeersMut;

use super::super::reference_state::ReferenceState;
use super::super::state_machine::merge_peer_blocks_into_primary;

// ─── Phase 6a — RefPeers (read side) ─────────────────────────────────

impl RefPeers for ReferenceState {
    fn peers_len(&self) -> usize {
        self.loro.peers_len()
    }

    fn peer_block_stable_ids(&self, peer_idx: usize) -> Vec<String> {
        self.loro.peer_block_stable_ids(peer_idx)
    }

    fn peer_modified_stable_ids(&self, peer_idx: usize) -> Vec<String> {
        self.loro.peer_modified_ids(peer_idx)
    }

    fn peer_block_content(&self, peer_idx: usize, stable_id: &str) -> Option<String> {
        self.loro.peer_block_content(peer_idx, stable_id)
    }

    fn peer_block_parent(&self, peer_idx: usize, stable_id: &str) -> Option<String> {
        self.loro.peer_block_parent(peer_idx, stable_id)
    }
}

// ─── Phase 6a — RefPeersMut (write side) ─────────────────────────────

impl RefPeersMut for ReferenceState {
    fn add_peer_from_primary_snapshot(&mut self) -> u64 {
        let layout = &self.domain.layout_blocks;
        self.loro.add_peer_from_primary_snapshot(
            &self.domain.block_state.blocks,
            &self.domain.block_state.block_documents,
            |id| layout.contains(id),
        )
    }

    fn peer_apply_create(
        &mut self,
        peer_idx: usize,
        parent_stable_id: Option<&str>,
        content: &str,
        stable_id: &str,
    ) {
        self.loro
            .peer_apply_create(peer_idx, parent_stable_id, content, stable_id);
    }

    fn peer_apply_update(&mut self, peer_idx: usize, stable_id: &str, content: &str) {
        self.loro.peer_apply_update(peer_idx, stable_id, content);
    }

    fn peer_apply_delete(&mut self, peer_idx: usize, stable_id: &str) {
        self.loro.peer_apply_delete(peer_idx, stable_id);
    }

    fn peer_apply_char_insert(&mut self, peer_idx: usize, stable_id: &str, pos: usize, text: &str) {
        self.loro
            .peer_apply_char_insert(peer_idx, stable_id, pos, text);
    }

    fn peer_apply_char_delete(&mut self, peer_idx: usize, stable_id: &str, pos: usize, len: usize) {
        self.loro
            .peer_apply_char_delete(peer_idx, stable_id, pos, len);
    }

    fn peer_sync_from_primary(&mut self, peer_idx: usize) {
        // Bidirectional sync: the Loro ext advances the shadow docs and hands
        // back the merge intent; the core merges it into the primary block tree
        // and re-canonicalizes; then the ext reflects post-merge primary state
        // back into the peer. Mirrors `sync_with_peer.rs::apply_to_ref`.
        let intent = self
            .loro
            .sync_peer_shadow(peer_idx, &self.domain.block_state.blocks);
        merge_peer_blocks_into_primary(
            &mut self.domain.block_state,
            &intent.peer_blocks,
            &intent.modified,
            &intent.created,
            self.loro.shadow_mesh.as_ref().expect("shadow mesh present"),
        );
        self.recanon_and_rebuild();
        let layout = &self.domain.layout_blocks;
        self.loro.reflect_primary_into_peer(
            peer_idx,
            &self.domain.block_state.blocks,
            &self.domain.block_state.block_documents,
            |id| layout.contains(id),
        );
    }

    fn peer_merge_into_primary(&mut self, peer_idx: usize) {
        // Unidirectional: peer→primary merge only, no reflect-back.
        // Mirrors `merge_from_peer.rs::apply_to_ref`.
        let intent = self
            .loro
            .merge_peer_shadow(peer_idx, &self.domain.block_state.blocks);
        merge_peer_blocks_into_primary(
            &mut self.domain.block_state,
            &intent.peer_blocks,
            &intent.modified,
            &intent.created,
            self.loro.shadow_mesh.as_ref().expect("shadow mesh present"),
        );
        self.recanon_and_rebuild();
    }
}

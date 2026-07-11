//! `RefPeers` / `RefPeersMut` (Phase 6a).

use holon_pbt_core::capabilities::RefPeers;
use holon_pbt_core::capabilities::RefPeersMut;

use super::super::peer_ops::PeerBlock;
use super::super::peer_ref_state::PeerRefState;
use super::super::reference_state::ReferenceState;
use super::super::state_machine::merge_peer_blocks_into_primary;

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
            self.shadow_mesh = Some(super::super::shadow_mesh::ShadowMesh::seeded_from_blocks(
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
        // Layout-classified source blocks (query/render machinery like
        // `journals::src::0` / `journals::render::0`) are excluded from the
        // peer model, mirroring the UI mutation arms' `no_content_update`
        // exclusion: peer content edits on them are not modeled (the oracle's
        // `render_expressions` would go stale), and a non-frontend wiring never
        // boots them into the global Loro doc at all — the SUT peer fork (a
        // snapshot of that doc) lacks the node and `peer_update_block` panics
        // (keystone fresh-case RED, 2026-07-11).
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
                !is_seed && !b.is_page() && !self.domain.layout_blocks.contains(&b.id)
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
        // sees post-merge truth). Layout machinery excluded — same rule as
        // the fork in `add_peer_from_primary_snapshot`.
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
                !is_seed && !b.is_page() && !self.domain.layout_blocks.contains(&b.id)
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

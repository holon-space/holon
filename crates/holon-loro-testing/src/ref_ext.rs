//! Loro-private reference-state extension (RefStateSplit Inc 5).
//!
//! The reference model's Loro-only state — peer instances, the E-solid shadow
//! CRDT mesh, and the Lamport `clock_feed` side-channel — factored out of the
//! central monolith into the subsystem crate that owns Loro (co-location north
//! star, `PbtCompositionDesign.md` §5.5). The composition root holds one
//! [`LoroRefExt`] field; the `RefPeers`/`RefPeersMut` cap impls stay in
//! integration-tests (orphan rule) as thin delegators.
//!
//! ## Split boundary — "the module hands intent; the core computes the merge"
//! [`LoroRefExt`] owns and drives everything Loro-private: it advances the
//! shadow docs, mutates the peer models, and *collects* the block-level merge
//! intent ([`PeerMergeIntent`]). It never touches the core `BlockState` — that
//! type lives in integration-tests, which this crate must not depend on (the
//! `no_ref_state_dep` guard). The merge into the primary block tree
//! (`merge_peer_blocks_into_primary`) and the `recanon_and_rebuild` fixpoint
//! stay core-side, invoked by the thin cap impl once this module has handed it
//! the intent. Primary block data flows IN through `holon_api`-typed parameters
//! (`&BTreeMap<EntityUri, Block>`) plus a layout predicate — never a
//! `ReferenceState`.
//!
//! ## `clock_feed` Clone seam (preserved from the pre-split home)
//! [`LoroRefExt::clock_feed`] is `Arc<Mutex<Option<u32>>>`: `Clone` SHARES the
//! cell — it is a harness seam, not model state. proptest clones the reference
//! per step and per case; the composed harness writes the SUT's scalar Lamport
//! height into this cell after boot and after every apply+settle
//! (`composed::harness::feed_sut_clock`), and the shadow primary is padded to
//! that height at each fork/sync/merge/primary-edit boundary. Because every
//! clone shares the same `Arc`, the height the harness feeds is visible to the
//! ref no matter which cloned generation reads it. Empty/stale during
//! proptest's generation phase, which is harmless (generation consumes no
//! clock-dependent predictions; execution re-evolves the ref fresh). Do NOT
//! replace the `Arc` with a plain value — that silently breaks the seam.
//!
//! ## `ShadowMesh` Clone cost (preserved)
//! [`LoroRefExt::shadow_mesh`] deep-forks every shadow Loro doc on `Clone`
//! (proptest clones per step and per case). This cost is unchanged by the move.

use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::Block;
use holon_api::EntityUri;

use crate::peer_ops::PeerBlock;
use crate::shadow_mesh::ShadowMesh;

/// Reference state for a Loro-only peer.
#[derive(Debug, Clone)]
pub struct PeerRefState {
    pub peer_id: u64,
    pub blocks: HashMap<String, PeerBlock>,
    /// Stable IDs this peer has deleted since its last sync with the
    /// primary. Propagated by `SyncWithPeer`/`MergeFromPeer` so the
    /// primary's reference block map reflects the delete the production
    /// controller just applied via `subscribe_root`.
    pub deleted_stable_ids: HashSet<String>,
    /// Stable IDs explicitly modified by PeerEdit::Update since AddPeer.
    /// Used by `merge_peer_blocks_into_primary` to distinguish peer edits
    /// from inherited-at-AddPeer blocks.
    pub modified_stable_ids: HashSet<String>,
    /// Stable IDs created by PeerEdit::Create since the last sync. Only
    /// these are added to the primary on merge — inherited-at-AddPeer
    /// blocks the primary may have since deleted must NOT be re-added,
    /// because the actual Loro CRDT keeps primary-side deletes.
    pub created_stable_ids: HashSet<String>,
}

/// The block-level merge intent [`LoroRefExt`] hands to the core after
/// advancing the shadow docs. The core (`merge_peer_blocks_into_primary`)
/// consumes it to mutate the primary `BlockState`; this module never touches
/// that type. `peer_blocks` arrives in `HashMap` iteration order — the merge is
/// responsible for its own deterministic stamping (it sorts by stable id).
pub struct PeerMergeIntent {
    pub peer_blocks: Vec<PeerBlock>,
    pub modified: HashSet<String>,
    pub created: HashSet<String>,
}

/// Loro-private extension of the reference model: peer instances + the E-solid
/// shadow mesh + the Lamport clock feed. See the module docs for the split
/// boundary and the two Clone seams (`clock_feed` shares the cell;
/// `shadow_mesh` deep-forks).
#[derive(Debug, Clone)]
pub struct LoroRefExt {
    /// Loro-only peer instances for multi-instance sync testing.
    pub peers: Vec<PeerRefState>,

    /// E-solid shadow Loro peer mesh — the oracle-side CRDT predictor for
    /// peer-merge outcomes (tie-break sibling order, concurrent-text
    /// interleaving). Created lazily at the first `AddPeer`, seeded from the
    /// ref block map at that moment. `Clone` deep-forks every shadow doc
    /// (proptest clones per step and per case). See [`crate::shadow_mesh`].
    pub shadow_mesh: Option<ShadowMesh>,

    /// Clock side-channel (the `IdResolver` pattern): the composed harness
    /// writes the SUT's scalar Lamport height
    /// (`SutLoroLog::loro_lamport_height`) here after every apply+settle
    /// (and once after build); the ref pads the shadow primary to it before
    /// boundary ops. `Clone` SHARES the cell — it is a harness seam, not
    /// model state (see module docs). Empty/stale during proptest's
    /// generation phase, which is harmless: generation consumes no
    /// clock-dependent predictions and execution re-evolves the ref fresh
    /// (padding is lenient — see `ShadowMesh::pad_primary_to`).
    pub clock_feed: Arc<Mutex<Option<u32>>>,
}

impl Default for LoroRefExt {
    fn default() -> Self {
        Self {
            peers: Vec::new(),
            shadow_mesh: None,
            clock_feed: Arc::new(Mutex::new(None)),
        }
    }
}

impl LoroRefExt {
    // ─── RefPeers read side (thin field reads; delegated from ref_caps/peers.rs)
    // ──

    pub fn peers_len(&self) -> usize {
        self.peers.len()
    }

    pub fn peer_block_stable_ids(&self, peer_idx: usize) -> Vec<String> {
        self.peers
            .get(peer_idx)
            .map(|p| p.blocks.keys().cloned().collect())
            .unwrap_or_default()
    }

    pub fn peer_modified_ids(&self, peer_idx: usize) -> Vec<String> {
        self.peers
            .get(peer_idx)
            .map(|p| p.modified_stable_ids.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn peer_block_content(&self, peer_idx: usize, stable_id: &str) -> Option<String> {
        self.peers
            .get(peer_idx)
            .and_then(|p| p.blocks.get(stable_id))
            .map(|b| b.content.clone())
    }

    pub fn peer_block_parent(&self, peer_idx: usize, stable_id: &str) -> Option<String> {
        self.peers
            .get(peer_idx)
            .and_then(|p| p.blocks.get(stable_id))
            .and_then(|b| b.parent_stable_id.clone())
    }

    /// Stable IDs any peer has modified. Aggregated across all peers (the root
    /// `ReferenceState::peer_modified_stable_ids` delegates here); JoinBlock
    /// excludes these to avoid edit/peer interleaving races.
    pub fn all_modified_stable_ids(&self) -> HashSet<String> {
        self.peers
            .iter()
            .flat_map(|p| p.modified_stable_ids.iter().cloned())
            .collect()
    }

    // ─── RefPeersMut write side (ported inherent logic) ──────────────────

    /// Fork a new shadow+model peer from the current primary snapshot. Reads
    /// primary block data through parameters; `is_layout_block` reports whether
    /// a block id is layout/query machinery (excluded from the peer model, same
    /// as the UI mutation arms' `no_content_update` exclusion). Returns the new
    /// peer id (`100 + idx`).
    pub fn add_peer_from_primary_snapshot(
        &mut self,
        primary_blocks: &BTreeMap<EntityUri, Block>,
        block_documents: &BTreeMap<EntityUri, EntityUri>,
        is_layout_block: impl Fn(&EntityUri) -> bool,
    ) -> u64 {
        // E-solid shadow mesh: lazily seed at the first AddPeer, mirror any
        // pending primary state, pad to the SUT's fed height (the height the
        // SUT's snapshot export sees), THEN fork — so the shadow peer's base
        // clock matches the SUT peer's (clock_parity_spike s3/s7: staggered
        // fork heights are exactly what an unpadded shadow gets wrong).
        if self.shadow_mesh.is_none() {
            self.shadow_mesh = Some(ShadowMesh::seeded_from_blocks(primary_blocks));
        }
        let mesh = self.shadow_mesh.as_mut().expect("just seeded");
        mesh.catch_up_primary(primary_blocks);
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
        let peer_blocks: HashMap<String, PeerBlock> = primary_blocks
            .values()
            .filter(|b| {
                let is_seed = block_documents
                    .get(&b.id)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                !is_seed && !b.is_page() && !is_layout_block(&b.id)
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
            deleted_stable_ids: HashSet::new(),
            modified_stable_ids: HashSet::new(),
            created_stable_ids: HashSet::new(),
        });
        peer_id
    }

    pub fn peer_apply_create(
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

    pub fn peer_apply_update(&mut self, peer_idx: usize, stable_id: &str, content: &str) {
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

    pub fn peer_apply_delete(&mut self, peer_idx: usize, stable_id: &str) {
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
    pub fn peer_apply_char_insert(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos: usize,
        text: &str,
    ) {
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

    pub fn peer_apply_char_delete(
        &mut self,
        peer_idx: usize,
        stable_id: &str,
        pos: usize,
        len: usize,
    ) {
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

    /// Bidirectional sync, shadow half: mirror pending primary state, pad to
    /// the SUT's fed height (the pre-sync boundary), run the REAL CRDT sync
    /// on the shadow docs, then hand back the block-level merge intent for
    /// the core to apply. Mirrors `sync_with_peer.rs::apply_to_ref`'s
    /// shadow-first ordering (the merge below CONSUMES the shadow's
    /// converged text + tie-break order instead of modeling them).
    pub fn sync_peer_shadow(
        &mut self,
        peer_idx: usize,
        primary_blocks: &BTreeMap<EntityUri, Block>,
    ) -> PeerMergeIntent {
        {
            let mesh = self
                .shadow_mesh
                .as_ref()
                .expect("shadow mesh exists once peers do");
            mesh.catch_up_primary(primary_blocks);
            if let Some(h) = *self.clock_feed.lock().expect("clock_feed lock") {
                mesh.pad_primary_to(h);
            }
            mesh.sync_peer_bidirectional(peer_idx);
        }
        self.take_merge_intent(peer_idx)
    }

    /// Unidirectional merge, shadow half: peer→primary only, no reflect-back.
    /// Mirrors `merge_from_peer.rs::apply_to_ref`.
    pub fn merge_peer_shadow(
        &mut self,
        peer_idx: usize,
        primary_blocks: &BTreeMap<EntityUri, Block>,
    ) -> PeerMergeIntent {
        {
            let mesh = self
                .shadow_mesh
                .as_ref()
                .expect("shadow mesh exists once peers do");
            mesh.catch_up_primary(primary_blocks);
            if let Some(h) = *self.clock_feed.lock().expect("clock_feed lock") {
                mesh.pad_primary_to(h);
            }
            mesh.merge_peer_into_primary(peer_idx);
        }
        self.take_merge_intent(peer_idx)
    }

    /// Snapshot the peer's modified/created sets and block map into a
    /// [`PeerMergeIntent`], then clear the peer's per-sync delta sets (the
    /// primary-side merge is now the core's job). Order of clears preserved
    /// byte-for-byte from the pre-split impl.
    fn take_merge_intent(&mut self, peer_idx: usize) -> PeerMergeIntent {
        let modified = self.peers[peer_idx].modified_stable_ids.clone();
        let created = self.peers[peer_idx].created_stable_ids.clone();
        self.peers[peer_idx].deleted_stable_ids.clear();
        self.peers[peer_idx].modified_stable_ids.clear();
        self.peers[peer_idx].created_stable_ids.clear();
        let peer_blocks: Vec<_> = self.peers[peer_idx].blocks.values().cloned().collect();
        PeerMergeIntent {
            peer_blocks,
            modified,
            created,
        }
    }

    /// Primary → peer reflect-back (bidirectional sync only, runs AFTER the
    /// core merge + `recanon_and_rebuild`): insert any non-seed, non-page
    /// primary block into the peer (overwrite content so the peer sees
    /// post-merge truth). Layout machinery excluded — same rule as the fork
    /// in [`Self::add_peer_from_primary_snapshot`].
    pub fn reflect_primary_into_peer(
        &mut self,
        peer_idx: usize,
        primary_blocks: &BTreeMap<EntityUri, Block>,
        block_documents: &BTreeMap<EntityUri, EntityUri>,
        is_layout_block: impl Fn(&EntityUri) -> bool,
    ) {
        let primary_as_peer: Vec<PeerBlock> = primary_blocks
            .values()
            .filter(|b| {
                let is_seed = block_documents
                    .get(&b.id)
                    .is_some_and(|doc| doc.is_no_parent() || doc.is_sentinel());
                !is_seed && !b.is_page() && !is_layout_block(&b.id)
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

    /// Shadow-catch-up read side (the root
    /// `ReferenceState::shadow_catch_up_primary` delegates here): pad the
    /// shadow primary to the latest fed SUT Lamport height, then mirror the
    /// ref block map into it. No-op until the mesh exists (first
    /// `AddPeer`).
    pub fn shadow_catch_up_primary(&self, primary_blocks: &BTreeMap<EntityUri, Block>) {
        let Some(mesh) = &self.shadow_mesh else {
            return;
        };
        if let Some(h) = *self.clock_feed.lock().expect("clock_feed lock") {
            mesh.pad_primary_to(h);
        }
        mesh.catch_up_primary(primary_blocks);
    }
}

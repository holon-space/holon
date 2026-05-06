//! Transition: bidirectional sync between primary's LoroDoc and a peer.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1537-1541` (generator),
//! `state_machine.rs:3513-3515` (precondition),
//! `state_machine.rs:2848-2919` (ref-state apply),
//! `sut.rs:4454-4476` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::peer_ops::PeerBlock;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::state_machine::{merge_peer_blocks_into_primary, refresh_peer_baseline};
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Bidirectional sync between primary's LoroDoc and a peer via DirectSync.
#[derive(Clone, Debug)]
pub struct SyncWithPeer {
    pub peer_idx: usize,
}

impl E2ETransitionFactory for SyncWithPeer {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (peer indices) and let `preconditions`
        // be the single source of truth for which ones are actually syncable.
        let candidates: Vec<usize> = (0..state.peers.len())
            .filter(|peer_idx| {
                SyncWithPeer {
                    peer_idx: *peer_idx,
                }
                .preconditions(state)
                .is_good()
            })
            .collect();
        check(!candidates.is_empty(), Reason::NoPeersAvailable).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|peer_idx| SyncWithPeer { peer_idx })
                .boxed();
            (2, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for SyncWithPeer {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(state.variant.enable_loro, Reason::LoroRequiredForPeers),
            check(
                self.peer_idx < state.peers.len(),
                Reason::PeerIndexOutOfBounds,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        // Peer → primary: apply creates/updates.
        // Peer deletes are tracked on `deleted_stable_ids` but
        // NOT propagated to the primary reference model. The
        // production path DOES propagate them (via
        // `LoroSyncController`'s `subscribe_root` → outbound
        // reconcile), but accurately mirroring Loro's cascading-
        // delete semantics in the reference model requires a
        // PBT-wide refactor — the ref model uses a different ID
        // convention than the peer's Loro tree. Known gap:
        // scenarios that combine `PeerEdit::Delete` with
        // `SyncWithPeer`/`MergeFromPeer` will flag divergence.
        let modified = state.peers[self.peer_idx].modified_stable_ids.clone();
        let created = state.peers[self.peer_idx].created_stable_ids.clone();
        let baseline = state.peers[self.peer_idx].baseline_contents.clone();
        state.peers[self.peer_idx].deleted_stable_ids.clear();
        state.peers[self.peer_idx].modified_stable_ids.clear();
        state.peers[self.peer_idx].created_stable_ids.clear();
        let peer_blocks: Vec<_> = state.peers[self.peer_idx]
            .blocks
            .values()
            .cloned()
            .collect();
        merge_peer_blocks_into_primary(
            &mut state.block_state,
            &peer_blocks,
            &modified,
            &created,
            &baseline,
        );
        // See the matching comment in `MergeFromPeer` above.
        state.recanon_and_rebuild();

        // Primary → peer: add any primary blocks missing from peer,
        // but skip blocks the peer deleted in this round (already removed
        // from primary above, so they won't be re-added).
        //
        // Also skip seed blocks (document blocks at sentinel:no_parent).
        // These live in the reference model for bookkeeping but never
        // reach Loro — the actual peer LoroDoc doesn't contain them.
        // Including them here would create a PeerBlock with
        // `stable_id = "no_parent"` (from the seed_doc_block's URI),
        // which a subsequent `MergeFromPeer` would synthesize back
        // into the primary ref model as a phantom `block:no_parent`.
        let primary_as_peer: Vec<_> = state
            .block_state
            .blocks
            .values()
            .filter(|b| {
                let is_seed = state
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
        let peer = &mut state.peers[self.peer_idx];
        for pb in primary_as_peer {
            // Bidirectional sync: both sides converge to the merged
            // primary state. Overwrite (not just `or_insert`) so the
            // peer's PeerBlock content reflects the post-merge truth.
            peer.blocks.insert(pb.stable_id.clone(), pb);
        }
        refresh_peer_baseline(peer);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_sync_with_peer(self.peer_idx).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        // SyncWithPeer: async CDC drain from previous transitions can land here.
        // In production, fires Loro's `subscribe_root` callback, which wakes
        // `LoroSyncController` to reconcile the diff into the command/event bus.
        ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

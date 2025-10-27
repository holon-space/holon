//! Transition: one-directional merge — peer's changes → primary.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1543-1547` (generator),
//! `state_machine.rs:3513-3515` (precondition, shared with SyncWithPeer),
//! `state_machine.rs:2820-2846` (ref-state apply),
//! `sut.rs:4478-4511` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::state_machine::{merge_peer_blocks_into_primary, refresh_peer_baseline};
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// One-directional merge: peer's changes → primary.
#[derive(Clone, Debug)]
pub struct MergeFromPeer {
    pub peer_idx: usize,
}

impl E2ETransitionFactory for MergeFromPeer {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started || !state.variant.enable_loro {
            return None;
        }
        if state.peers.is_empty() {
            return None;
        }
        let peer_count = state.peers.len();
        let strat = (0..peer_count)
            .prop_map(|peer_idx| MergeFromPeer { peer_idx })
            .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for MergeFromPeer {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started && self.peer_idx < state.peers.len()
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
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
        // Newly-created peer blocks are inserted with the default
        // `sequence=0` (via `Block::default()` in `from_block_content`),
        // colliding with whatever sequence the parent's existing
        // children already have. Production renders by
        // `(content_type group, sort_key, id)` and re-parses sequences
        // 0..N in render order — so the parent's child ordering only
        // converges after a recanon. Without this, the assertion at
        // `assertions.rs:117` flags an order mismatch on the next
        // org-file round-trip.
        state.recanon_and_rebuild();
        refresh_peer_baseline(&mut state.peers[self.peer_idx]);
    }

    async fn apply_to_sut(&self, _ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_merge_from_peer(self.peer_idx).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        // MergeFromPeer: async CDC drain from previous transitions can land here.
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

//! Transition: one-directional merge — peer's changes → primary.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1543-1547` (generator),
//! `state_machine.rs:3513-3515` (precondition, shared with SyncWithPeer),
//! `state_machine.rs:2820-2846` (ref-state apply),
//! `sut.rs:4478-4511` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// One-directional merge: peer's changes → primary.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MergeFromPeer {
    pub peer_idx: usize,
}

impl TransitionFactory<ReferenceState> for MergeFromPeer {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (peer indices) and let `preconditions` be the
        // single source of truth for which ones are actually mergeable. Avoids
        // duplicating the Loro / peer count checks across two sites.
        let candidates: Vec<usize> = (0..state.peers.len())
            .filter(|peer_idx| {
                MergeFromPeer {
                    peer_idx: *peer_idx,
                }
                .preconditions(state)
                .is_good()
            })
            .collect();
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|peer_idx| MergeFromPeer { peer_idx })
                .boxed();
            (1, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for MergeFromPeer {
    type Reason = Reason;

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
        use holon_pbt_core::capabilities::RefPeersMut;
        // recanon_and_rebuild + refresh_peer_baseline are handled
        // inside `RefPeersMut::peer_merge_into_primary` — the order
        // matters because newly-created peer blocks default to
        // sequence=0 and need a recanon pass before the next org
        // round-trip (see `assertions.rs:117`).
        state.peer_merge_into_primary(self.peer_idx);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for MergeFromPeer {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_merge_from_peer(self.peer_idx).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for MergeFromPeer {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
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

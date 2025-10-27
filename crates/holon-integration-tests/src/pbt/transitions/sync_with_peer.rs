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

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Bidirectional sync between primary's LoroDoc and a peer via DirectSync.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SyncWithPeer {
    pub peer_idx: usize,
}

impl TransitionFactory<ReferenceState> for SyncWithPeer {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutLoro,
        >()]
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Loro)
    }

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

impl TransitionRef<ReferenceState> for SyncWithPeer {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(state.enable_loro(), Reason::LoroRequiredForPeers),
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
        // Known gap: peer deletes aren't propagated to primary — see
        // the original comment block in git history. Mirror logic now
        // lives in `RefPeersMut::peer_sync_from_primary`.
        state.peer_sync_from_primary(self.peer_idx);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for SyncWithPeer {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_sync_with_peer(self.peer_idx).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for SyncWithPeer {
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

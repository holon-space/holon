//! Transition: bidirectional sync between primary's LoroDoc and a peer.
//!
//! @pbt rung external
//!   bidirectional CRDT sync between primary and peer.
//! @pbt covers loro-bidi-sync — two-way LoroDoc convergence
//!
//! Mirrors the legacy logic split across `state_machine.rs:1537-1541`
//! (generator), `state_machine.rs:3513-3515` (precondition),
//! `state_machine.rs:2848-2919` (ref-state apply),
//! `sut.rs:4454-4476` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
#[cfg(feature = "otel-testing")]
use holon_pbt_core::budget::ExpectedSql;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefPeers;
use holon_pbt_core::capabilities::RefPeersMut;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

/// Bidirectional sync between primary's LoroDoc and a peer via DirectSync.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I sync with peer {peer_idx}")]
pub struct SyncWithPeer {
    pub peer_idx: usize,
}

impl<R: RefLifecycle + RefPeers + RefPeersMut> TransitionFactory<R> for SyncWithPeer {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Loro)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (peer indices) and let `preconditions`
        // be the single source of truth for which ones are actually syncable.
        let candidates: Vec<usize> = (0..state.peers_len())
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

impl<R: RefLifecycle + RefPeers + RefPeersMut> TransitionRef<R> for SyncWithPeer {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.enable_loro(), Reason::LoroRequiredForPeers),
            check(
                self.peer_idx < state.peers_len(),
                Reason::PeerIndexOutOfBounds,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Known gap: peer deletes aren't propagated to primary — see
        // the original comment block in git history. Mirror logic now
        // lives in `RefPeersMut::peer_sync_from_primary`.
        state.peer_sync_from_primary(self.peer_idx);
    }
}

holon_pbt_core::cap_transition! {
    SyncWithPeer: holon_pbt_core::capabilities::SutLoro,
    where R: [ RefLifecycle + RefPeers + RefPeersMut ],
    |me, _state, sut| {
        sut.apply_sync_with_peer(me.peer_idx).await;
    }
    sql_budget: |_me, state| {
        // Pushing the primary's state at a peer touches no SQL: 297 samples,
        // all 0 reads (all at d=1 — the multi-doc arm is unsampled, so the
        // shared CDC drain is carried on the same terms as its siblings). In
        // production this fires Loro's `subscribe_root` callback, which wakes
        // `LoroSyncController` to reconcile the diff into the command/event bus.
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()),
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

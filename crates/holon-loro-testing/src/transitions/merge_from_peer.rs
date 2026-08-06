//! Transition: one-directional merge — peer's changes → primary.
//!
//! @pbt rung external
//!   one-directional CRDT merge (peer -> primary), no UI path.
//! @pbt covers loro-merge-oneway — one-way peer delta import
//!
//! Mirrors the legacy logic split across `state_machine.rs:1543-1547`
//! (generator), `state_machine.rs:3513-3515` (precondition, shared with
//! SyncWithPeer), `state_machine.rs:2820-2846` (ref-state apply),
//! `sut.rs:4478-4511` (SUT apply), and
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

/// One-directional merge: peer's changes → primary.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MergeFromPeer {
    pub peer_idx: usize,
}

impl<R: RefLifecycle + RefPeers + RefPeersMut> TransitionFactory<R> for MergeFromPeer {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Loro)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (peer indices) and let `preconditions` be the
        // single source of truth for which ones are actually mergeable. Avoids
        // duplicating the Loro / peer count checks across two sites.
        let candidates: Vec<usize> = (0..state.peers_len())
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

impl<R: RefLifecycle + RefPeers + RefPeersMut> TransitionRef<R> for MergeFromPeer {
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
        // recanon_and_rebuild is handled inside
        // `RefPeersMut::peer_merge_into_primary` — the order matters
        // because newly-created peer blocks default to sequence=0 and
        // need a recanon pass before the next org round-trip (see
        // `assertions.rs:117`).
        state.peer_merge_into_primary(self.peer_idx);
    }
}

holon_pbt_core::cap_transition! {
    MergeFromPeer: holon_pbt_core::capabilities::SutLoro,
    where R: [ RefLifecycle + RefPeers + RefPeersMut ],
    |me, _state, sut| {
        sut.apply_merge_from_peer(me.peer_idx).await;
    }
    sql_budget: |_me, state| {
        // The merge itself costs 5 reads (block hydrate + the doc-scoped
        // descendants CTE) on top of the shared CDC drain: 137 samples, 0 at
        // d=1 (n=134) and 8 at d=3 (n=3) = 3 floor + 5. In production this
        // fires Loro's `subscribe_root` callback, which wakes
        // `LoroSyncController` to reconcile the diff into the command/event bus.
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()) + 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

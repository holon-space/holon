//! Transition: add a Loro-only peer instance.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1431-1433`
//! (generator), `state_machine.rs:3491-3492` (precondition),
//! `state_machine.rs:2738-2786` (ref-state apply),
//! `sut.rs:4365-4390` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefPeers;
use holon_pbt_core::capabilities::RefPeersMut;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
use crate::pbt::transition_dispatch::SutHandle;
use crate::pbt::validation::Reason;
use crate::pbt::validation::check;

/// Add a Loro-only peer instance that shares the primary's current state.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct AddPeer;

// ── Capability-bound free functions (Phase 6a) ────────────────────

pub fn add_peer_preconditions<R: RefPeers + RefLifecycle>(state: &R) -> Validated<(), Reason> {
    let checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.enable_loro(), Reason::LoroRequiredForPeers),
        check(state.peers_len() < 3, Reason::PeerLimitReached),
    ];
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

impl<R: RefPeers + RefLifecycle> TransitionFactory<R> for AddPeer {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutLoro,
        >()]
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Loro)
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        add_peer_preconditions(state).map(|_| (1, Just(AddPeer).boxed()))
    }
}

impl<R: RefPeers + RefPeersMut + RefLifecycle> TransitionRef<R> for AddPeer {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        add_peer_preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.add_peer_from_primary_snapshot();
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for AddPeer {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_add_peer().await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for AddPeer {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        // AddPeer: export_snapshot triggers ~5 SQL reads (store persistence).
        // Others: async CDC drain from previous transitions can land here.
        ExpectedSql {
            reads: 5,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

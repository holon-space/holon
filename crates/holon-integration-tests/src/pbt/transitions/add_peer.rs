//! Transition: add a Loro-only peer instance.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1431-1433` (generator),
//! `state_machine.rs:3491-3492` (precondition),
//! `state_machine.rs:2738-2786` (ref-state apply),
//! `sut.rs:4365-4390` (SUT apply), and
//! `transition_budgets.rs:351-360` (expected SQL).

use holon_pbt_core::capabilities::{RefLifecycle, RefPeers, RefPeersMut};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

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

pub fn add_peer_weighted_generator<R: RefPeers + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<AddPeer>), Reason> {
    add_peer_preconditions(state).map(|_| (1, Just(AddPeer).boxed()))
}

pub fn add_peer_apply_to_ref<R: RefPeersMut>(state: &mut R) {
    state.add_peer_from_primary_snapshot();
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl E2ETransitionFactory for AddPeer {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        add_peer_weighted_generator(state)
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for AddPeer {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        add_peer_preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        add_peer_apply_to_ref(state);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_add_peer().await;
    }

    #[cfg(feature = "otel-testing")]
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

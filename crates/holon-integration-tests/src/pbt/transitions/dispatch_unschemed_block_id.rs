//! Transition: dispatch a block write whose `id` param carries no scheme.
//!
//! @pbt rung dispatch
//!   `assert_unschemed_block_id_refused` is a dispatcher-level assertion probe.
//! @pbt covers operation-boundary-refuses-unschemed-entity-reference — the id
//!   form that let the write leg and the read legs key on different strings
//!
//! The refusal is a property of the id FORM alone, so the transition names a
//! block that does not exist: it is drawable on every seed, and the assertion
//! tells the boundary refusal apart from the "block does not exist" answer a
//! reachable id would produce. The assertion lives in the SUT apply
//! (`assert_unschemed_block_id_refused`) — the transition IS the teeth, as in
//! `EpochFlipRejected`.

use holon_pbt_core::RequiredWiring;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutUnschemedIdDispatch;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Aim a user-origin `set_field("content")` at the unschemed block id
/// `bare_id`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I dispatch a block write addressed by the unschemed id {bare_id}")]
pub struct DispatchUnschemedBlockId {
    pub bare_id: String,
}

impl<R: RefLifecycle> TransitionFactory<R> for DispatchUnschemedBlockId {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn required_wiring() -> RequiredWiring {
        RequiredWiring::Any
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        check(state.app_started(), Reason::AppNotStarted).map(|()| {
            let strat = proptest::string::string_regex("unschemed-probe-[a-z]{1,6}")
                .expect("valid regex")
                .prop_map(|bare_id| DispatchUnschemedBlockId { bare_id })
                .boxed();
            // Low weight: one refused dispatch per run arms the rung, and every
            // draw spent here is a draw not spent on the editing alphabet.
            (2, strat)
        })
    }
}

impl<R: RefLifecycle> TransitionRef<R> for DispatchUnschemedBlockId {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(state.app_started(), Reason::AppNotStarted)
    }

    fn apply_to_ref(&self, _: &mut R) {
        // A refused write changes nothing; every block invariant then compares
        // the store against a reference the transition left alone.
    }
}

crate::cap_transition! {
    DispatchUnschemedBlockId: SutUnschemedIdDispatch,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.assert_unschemed_block_id_refused(&me.bare_id).await;
    }
    sql_budget: |_me, _state| {
        // The boundary answers above every gate and every provider: the params
        // are parsed, the dispatch dies, no statement is issued.
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 4,
        }
    }
}

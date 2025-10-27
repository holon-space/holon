//! Transition: no-op transition for search space exploration.
//!
//! Mirrors the legacy logic from `state_machine.rs:3075` (precondition),
//! `state_machine.rs:1736` (ref-state apply), and
//! `transition_budgets.rs:108-113` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::validation::Reason;
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// No-op transition: does nothing to either the reference model or SUT.
/// Enables the PBT to explore the search space without making progress,
/// useful for validating invariants in a stable state.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Nothing;

impl TransitionFactory<ReferenceState> for Nothing {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Nothing.preconditions(state).map(|_| {
            let strat = Just(Nothing).boxed();
            (1, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for Nothing {
    type Reason = Reason;

    fn preconditions(&self, _: &ReferenceState) -> Validated<(), Reason> {
        Validated::Good(())
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // No-op: reference state is unchanged
    }
}

// No-op SUT dispatch (no cap → bound on the full `SutHandle` bundle; `required_caps`
// stays the trait default of empty).
crate::cap_transition! {
    Nothing,
    |_me, _state, _sut| {}
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for Nothing {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}

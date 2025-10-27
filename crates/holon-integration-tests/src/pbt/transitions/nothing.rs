//! Transition: no-op transition for search space exploration.
//!
//! Mirrors the legacy logic from `state_machine.rs:3075` (precondition),
//! `state_machine.rs:1736` (ref-state apply), and
//! `transition_budgets.rs:108-113` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// No-op transition: does nothing to either the reference model or SUT.
/// Enables the PBT to explore the search space without making progress,
/// useful for validating invariants in a stable state.
#[derive(Clone, Debug)]
pub struct Nothing;

impl E2ETransitionFactory for Nothing {
    fn weighted_generator(_state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        // Nothing is always enabled with weight 1
        Some((1, Just(Nothing).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for Nothing {
    fn preconditions(&self, _state: &ReferenceState) -> bool {
        true
    }

    fn apply_to_ref(&self, _state: &mut ReferenceState) {
        // No-op: reference state is unchanged
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, _sut: &mut dyn SutHandle) {
        // No-op: SUT is unchanged
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}

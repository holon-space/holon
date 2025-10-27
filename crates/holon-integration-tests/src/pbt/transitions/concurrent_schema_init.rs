//! Transition: concurrent schema init stress test (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:862-868` (generator),
//! `state_machine.rs:3190-3194` (precondition),
//! `state_machine.rs:2481-2484` (ref-state apply),
//! `sut.rs:1849-1940` (SUT apply), and
//! `transition_budgets.rs:253-259` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Test that sequential schema operations don't cause database lock errors.
#[derive(Clone, Debug)]
pub struct ConcurrentSchemaInit;

impl E2ETransitionFactory for ConcurrentSchemaInit {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started
            || state.block_state.blocks.is_empty()
            || state.active_watches.is_empty()
        {
            return None;
        }
        Some((1, Just(ConcurrentSchemaInit).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for ConcurrentSchemaInit {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started
            && !state.block_state.blocks.is_empty()
            && !state.active_watches.is_empty()
    }

    fn apply_to_ref(&self, _state: &mut ReferenceState) {
        // ConcurrentSchemaInit doesn't change reference state - it only tests
        // that the database doesn't get locked when schema init runs concurrently.
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_concurrent_schema_init().await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 100,
            writes: 30,
            ddl: 250,
            tolerance: 50,
        }
    }
}

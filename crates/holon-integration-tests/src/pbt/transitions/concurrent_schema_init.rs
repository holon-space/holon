//! Transition: concurrent schema init stress test (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:862-868` (generator),
//! `state_machine.rs:3190-3194` (precondition),
//! `state_machine.rs:2481-2484` (ref-state apply),
//! `sut.rs:1849-1940` (SUT apply), and
//! `transition_budgets.rs:253-259` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Test that sequential schema operations don't cause database lock errors.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConcurrentSchemaInit;

impl E2ETransitionFactory for ConcurrentSchemaInit {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        ConcurrentSchemaInit
            .preconditions(state)
            .map(|()| (1, Just(ConcurrentSchemaInit).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for ConcurrentSchemaInit {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(
                !state.block_state.blocks.is_empty(),
                Reason::BlockStateEmpty,
            ),
            check(!state.active_watches.is_empty(), Reason::NoWatchesActive),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // ConcurrentSchemaInit doesn't change reference state - it only tests
        // that the database doesn't get locked when schema init runs concurrently.
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_concurrent_schema_init().await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 100,
            writes: 30,
            ddl: 250,
            tolerance: 50,
        }
    }
}

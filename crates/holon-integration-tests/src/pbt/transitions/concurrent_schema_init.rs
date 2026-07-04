//! Transition: concurrent schema init stress test (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:862-868` (generator),
//! `state_machine.rs:3190-3194` (precondition),
//! `state_machine.rs:2481-2484` (ref-state apply),
//! `sut.rs:1849-1940` (SUT apply), and
//! `transition_budgets.rs:253-259` (expected SQL).

use holon_pbt_core::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use holon_pbt_core::capabilities::{RefLayout, RefLifecycle, RefWatch, SutAppLifecycle};
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Test that sequential schema operations don't cause database lock errors.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ConcurrentSchemaInit;

impl<R: RefLifecycle + RefLayout + RefWatch> TransitionFactory<R> for ConcurrentSchemaInit {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: this exercises concurrent Turso IVM schema/matview init
        // via `ctx.engine()`; there is no engine in the no-Turso wiring.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        ConcurrentSchemaInit
            .preconditions(state)
            .map(|()| (1, Just(ConcurrentSchemaInit).boxed()))
    }
}

impl<R: RefLifecycle + RefLayout + RefWatch> TransitionRef<R> for ConcurrentSchemaInit {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!state.all_block_ids().is_empty(), Reason::BlockStateEmpty),
            check(!state.active_watch_ids().is_empty(), Reason::NoWatchesActive),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut R) {
        // ConcurrentSchemaInit doesn't change reference state - it only tests
        // that the database doesn't get locked when schema init runs concurrently.
    }
}

crate::cap_transition! {
    ConcurrentSchemaInit: SutAppLifecycle,
    where R: [ RefLifecycle + RefLayout + RefWatch ],
    |_me, _state, sut| {
        sut.concurrent_schema_init().await;
    }
    sql_budget: |_me, _state| {
        ExpectedSql {
            reads: 100,
            writes: 30,
            ddl: 250,
            tolerance: 50,
        }
    }
}

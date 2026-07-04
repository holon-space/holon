//! Transition: simulate app restart.
//!
//! Mirrors the legacy logic split across `state_machine.rs:825-830`
//! (generator), `state_machine.rs:3184-3186` (precondition),
//! `state_machine.rs:2453-2456` (ref-state apply),
//! `sut.rs:1497-1502` (SUT apply), and
//! `transition_budgets.rs:210-215` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Simulate an app restart: clears last_projection and triggers re-sync.
/// Blocks are preserved; the system re-processes files from disk.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SimulateRestart;

impl<R: RefLifecycle + RefLayout> TransitionFactory<R> for SimulateRestart {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: restart reopens the Turso DB from disk and waits on the
        // CDC accumulator via `ctx.engine()`. The no-Turso restart story (rebuild
        // the Loro container + re-ingest org) is out of scope for a1.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        SimulateRestart
            .preconditions(state)
            .map(|()| (1, Just(SimulateRestart).boxed()))
    }
}

impl<R: RefLifecycle + RefLayout> TransitionRef<R> for SimulateRestart {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!state.all_block_ids().is_empty(), Reason::BlockStateEmpty),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, _: &mut R) {
        // SimulateRestart doesn't change reference state - blocks should be
        // preserved. The SUT will clear last_projection and trigger
        // file re-processing.
    }
}

crate::cap_transition! {
    SimulateRestart: SutAppLifecycle,
    where R: [ RefLifecycle + RefLayout ],
    |_me, _state, sut| {
        sut.simulate_restart().await;
    }
    sql_budget: |_me, state| {
        ExpectedSql {
            reads: REACTIVE_BASE + 4,
            writes: 2,
            ddl: 0,
            tolerance: 3 + docs_tolerance(state),
        }
    }
}

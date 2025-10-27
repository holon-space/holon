//! Transition: simulate app restart.
//!
//! Mirrors the legacy logic split across `state_machine.rs:825-830` (generator),
//! `state_machine.rs:3184-3186` (precondition),
//! `state_machine.rs:2453-2456` (ref-state apply),
//! `sut.rs:1497-1502` (SUT apply), and
//! `transition_budgets.rs:210-215` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

/// Simulate an app restart: clears last_projection and triggers re-sync.
/// Blocks are preserved; the system re-processes files from disk.
#[derive(Clone, Debug)]
pub struct SimulateRestart;

impl E2ETransitionFactory for SimulateRestart {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started || state.block_state.blocks.is_empty() {
            return None;
        }

        Some((1, Just(SimulateRestart).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for SimulateRestart {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started && !state.block_state.blocks.is_empty()
    }

    fn apply_to_ref(&self, _state: &mut ReferenceState) {
        // SimulateRestart doesn't change reference state - blocks should be preserved.
        // The SUT will clear last_projection and trigger file re-processing.
    }

    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_simulate_restart(state).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + 4,
            writes: 2,
            ddl: 0,
            tolerance: 3 + docs_tolerance(state),
        }
    }
}

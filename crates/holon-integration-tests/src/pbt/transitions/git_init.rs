//! Transition: initialize a git repository.
//!
//! Mirrors the legacy logic split across `state_machine.rs:363-365` (generator),
//! `state_machine.rs:3103` (precondition),
//! `state_machine.rs:1935-1937` (ref-state apply),
//! `sut.rs:680-689` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Initialize git repository (runs `git init`).
#[derive(Clone, Debug)]
pub struct GitInit;

impl E2ETransitionFactory for GitInit {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if state.app_started {
            return None;
        }

        let vcs_weight = if !state.git_initialized && !state.jj_initialized {
            1
        } else {
            0
        };

        if vcs_weight == 0 || state.git_initialized {
            return None;
        }

        Some((vcs_weight, Just(GitInit).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for GitInit {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        !state.app_started && !state.git_initialized
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.git_initialized = true;
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_git_init().await;
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

//! Transition: initialize a git repository.
//!
//! Mirrors the legacy logic split across `state_machine.rs:363-365` (generator),
//! `state_machine.rs:3103` (precondition),
//! `state_machine.rs:1935-1937` (ref-state apply),
//! `sut.rs:680-689` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Initialize git repository (runs `git init`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GitInit;

impl E2ETransitionFactory for GitInit {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        GitInit
            .preconditions(state)
            .map(|_| (1, Just(GitInit).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for GitInit {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started, Reason::AppAlreadyStarted),
            check(!state.git_initialized, Reason::VcsAlreadyInitialized),
            check(!state.jj_initialized, Reason::VcsAlreadyInitialized),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.git_initialized = true;
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_git_init().await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}

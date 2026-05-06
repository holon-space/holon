//! Transition: initialize a jj repository.
//!
//! Mirrors the legacy logic split across `state_machine.rs:367-369` (generator),
//! `state_machine.rs:3104` (precondition),
//! `state_machine.rs:1938-1941` (ref-state apply),
//! `sut.rs:691-700` (SUT apply), and
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

/// Initialize jj repository (runs `jj git init`).
#[derive(Clone, Debug)]
pub struct JjGitInit;

impl E2ETransitionFactory for JjGitInit {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        JjGitInit
            .preconditions(state)
            .map(|_| (1, Just(JjGitInit).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for JjGitInit {
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
        state.jj_initialized = true;
        state.git_initialized = true; // jj git init also creates .git
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_jj_git_init().await;
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

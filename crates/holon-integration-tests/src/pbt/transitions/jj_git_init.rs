//! Transition: initialize a jj repository.
//!
//! Mirrors the legacy logic split across `state_machine.rs:367-369` (generator),
//! `state_machine.rs:3104` (precondition),
//! `state_machine.rs:1938-1941` (ref-state apply),
//! `sut.rs:691-700` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use holon_pbt_core::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use holon_pbt_core::capabilities::{RefBootMut, RefLifecycle, SutFixtureFs};
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Initialize jj repository (runs `jj git init`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct JjGitInit;

impl<R: RefLifecycle + RefBootMut> TransitionFactory<R> for JjGitInit {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        JjGitInit
            .preconditions(state)
            .map(|_| (1, Just(JjGitInit).boxed()))
    }
}

impl<R: RefLifecycle + RefBootMut> TransitionRef<R> for JjGitInit {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started(), Reason::AppAlreadyStarted),
            check(!state.git_initialized(), Reason::VcsAlreadyInitialized),
            check(!state.jj_initialized(), Reason::VcsAlreadyInitialized),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.mark_jj_initialized();
    }
}

crate::cap_transition! {
    JjGitInit: SutFixtureFs,
    where R: [ RefLifecycle + RefBootMut ],
    |_me, _state, sut| {
        sut.jj_git_init().await;
    }
    sql_budget: |_me, _state| {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}

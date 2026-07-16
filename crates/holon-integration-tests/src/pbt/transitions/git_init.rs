//! Transition: initialize a git repository.
//!
//! @pbt rung external
//! @pbt covers git-init — git repository initialization
//!
//! Mirrors the legacy logic split across `state_machine.rs:363-365`
//! (generator), `state_machine.rs:3103` (precondition),
//! `state_machine.rs:1935-1937` (ref-state apply),
//! `sut.rs:680-689` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBootMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutFixtureFs;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Initialize git repository (runs `git init`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GitInit;

impl<R: RefLifecycle + RefBootMut> TransitionFactory<R> for GitInit {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        GitInit
            .preconditions(state)
            .map(|_| (1, Just(GitInit).boxed()))
    }
}

impl<R: RefLifecycle + RefBootMut> TransitionRef<R> for GitInit {
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
        state.mark_git_initialized();
    }
}

crate::cap_transition! {
    GitInit: SutFixtureFs,
    where R: [ RefLifecycle + RefBootMut ],
    |_me, _state, sut| {
        sut.git_init().await;
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

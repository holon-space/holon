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

use crate::pbt::local_caps::SutFixtureFs;
use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Initialize git repository (runs `git init`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct GitInit;

impl TransitionFactory<ReferenceState> for GitInit {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn crate::pbt::local_caps::SutFixtureFs,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        GitInit
            .preconditions(state)
            .map(|_| (1, Just(GitInit).boxed()))
    }
}

impl TransitionRef<ReferenceState> for GitInit {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.action.app_started, Reason::AppAlreadyStarted),
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
}

#[allow(async_fn_in_trait)]
impl<S: SutFixtureFs> TransitionImpl<ReferenceState, S> for GitInit {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.git_init().await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for GitInit {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}

//! Transition: create a directory in the temp workspace.
//!
//! @pbt rung external
//! @pbt covers workspace-dir-create — fs directory creation in the temp
//! workspace
//!
//! Mirrors the legacy logic split across `state_machine.rs:354-361`
//! (generator), `state_machine.rs:3102` (precondition),
//! `state_machine.rs:1932-1934` (ref-state apply),
//! `sut.rs:672-678` (SUT apply), and
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

/// Create a directory (possibly nested) before app starts.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I create directory {path}")]
pub struct CreateDirectory {
    pub path: String,
}

impl<R: RefLifecycle + RefBootMut> TransitionFactory<R> for CreateDirectory {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Test the preconditions on a dummy instance to ensure state is valid
        // for creating any directory; the specific path is generated randomly.
        CreateDirectory {
            path: String::new(),
        }
        .preconditions(state)
        .map(|_| {
            let strat = crate::pbt::generators::generate_directory_path()
                .prop_map(|path| CreateDirectory { path })
                .boxed();
            (2, strat)
        })
    }
}

impl<R: RefLifecycle + RefBootMut> TransitionRef<R> for CreateDirectory {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started(), Reason::AppAlreadyStarted),
            check(
                state.pre_startup_directory_count() < 10,
                Reason::DirectoryLimitReached,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.push_pre_startup_directory(&self.path);
    }
}

crate::cap_transition! {
    CreateDirectory: SutFixtureFs,
    where R: [ RefLifecycle + RefBootMut ],
    |me, _state, sut| {
        sut.create_directory(&me.path).await;
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

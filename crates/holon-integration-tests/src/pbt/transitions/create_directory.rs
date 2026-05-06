//! Transition: create a directory in the temp workspace.
//!
//! Mirrors the legacy logic split across `state_machine.rs:354-361` (generator),
//! `state_machine.rs:3102` (precondition),
//! `state_machine.rs:1932-1934` (ref-state apply),
//! `sut.rs:672-678` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Create a directory (possibly nested) before app starts.
#[derive(Clone, Debug)]
pub struct CreateDirectory {
    pub path: String,
}

impl E2ETransitionFactory for CreateDirectory {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
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

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for CreateDirectory {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(!state.app_started, Reason::AppAlreadyStarted),
            check(
                state.pre_startup_directories.len() < 10,
                Reason::DirectoryLimitReached,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.pre_startup_directories.push(self.path.clone());
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_create_directory(&self.path).await;
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

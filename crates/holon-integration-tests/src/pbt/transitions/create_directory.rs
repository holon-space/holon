//! Transition: create a directory in the temp workspace.
//!
//! Mirrors the legacy logic split across `state_machine.rs:354-361` (generator),
//! `state_machine.rs:3102` (precondition),
//! `state_machine.rs:1932-1934` (ref-state apply),
//! `sut.rs:672-678` (SUT apply), and
//! `transition_budgets.rs:116-125` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// Create a directory (possibly nested) before app starts.
#[derive(Clone, Debug)]
pub struct CreateDirectory {
    pub path: String,
}

impl E2ETransitionFactory for CreateDirectory {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if state.app_started {
            return None;
        }

        let dir_count = state.pre_startup_directories.len();
        let dir_weight = if dir_count < 10 { 2 } else { 0 };

        if dir_weight == 0 {
            return None;
        }

        let strat = crate::pbt::generators::generate_directory_path()
            .prop_map(|path| CreateDirectory { path })
            .boxed();

        Some((dir_weight, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for CreateDirectory {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        !state.app_started
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.pre_startup_directories.push(self.path.clone());
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_create_directory(&self.path).await;
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

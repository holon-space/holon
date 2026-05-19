//! Transition: switch the current view (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:544-553` (generator),
//! `state_machine.rs:3164` (precondition),
//! `state_machine.rs:2219-2221` (ref-state apply),
//! `sut.rs:1323-1325` (SUT apply), and
//! `transition_budgets.rs:137-143` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

/// Switch the current view (e.g. "all", "sidebar", "main").
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SwitchView {
    pub view_name: String,
}

impl E2ETransitionFactory for SwitchView {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Enumerate parameter space (fixed view names) and let `preconditions`
        // be the single source of truth for which ones are actually switchable.
        let candidates: Vec<String> =
            vec!["all".to_string(), "sidebar".to_string(), "main".to_string()]
                .into_iter()
                .filter(|view_name| {
                    SwitchView {
                        view_name: view_name.clone(),
                    }
                    .preconditions(state)
                    .is_good()
                })
                .collect();
        check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = prop::sample::select(candidates)
                .prop_map(|view_name| SwitchView { view_name })
                .boxed();
            (1, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for SwitchView {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started, Reason::AppNotStarted)];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.current_view = self.view_name.clone();
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_switch_view(&self.view_name).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

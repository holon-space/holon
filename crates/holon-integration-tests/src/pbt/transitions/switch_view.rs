//! Transition: switch the current view (post-startup).
//!
//! @pbt rung dispatch
//!   VACUOUS (audit TR-VAC): `switch_view` is a pure harness field write
//!   (`*current_view.lock() = name`) that no production view-switch path
//!   drives — the oracle reads back the same field. Tests nothing of prod.
//! @pbt covers view-switch — current-view selection (harness mirror only; no
//! prod path)
//!
//! Mirrors the legacy logic split across `state_machine.rs:544-553`
//! (generator), `state_machine.rs:3164` (precondition),
//! `state_machine.rs:2219-2221` (ref-state apply),
//! `sut.rs:1323-1325` (SUT apply), and
//! `transition_budgets.rs:137-143` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefViewSelectionMut;
use holon_pbt_core::capabilities::SutViewControl;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Switch the current view (e.g. "all", "sidebar", "main").
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I switch to view {view_name}")]
pub struct SwitchView {
    pub view_name: String,
}

impl<R: RefLifecycle + RefViewSelectionMut> TransitionFactory<R> for SwitchView {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
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

impl<R: RefLifecycle + RefViewSelectionMut> TransitionRef<R> for SwitchView {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started(), Reason::AppNotStarted)];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.set_current_view(&self.view_name);
    }
}

crate::cap_transition! {
    SwitchView: SutViewControl,
    where R: [ RefLifecycle + RefViewSelectionMut ],
    |me, _state, sut| {
        sut.switch_view(&me.view_name).await;
    }
    sql_budget: |_me, state| {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

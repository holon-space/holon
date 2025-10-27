//! Transition: switch the current view (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:544-553`
//! (generator), `state_machine.rs:3164` (precondition),
//! `state_machine.rs:2219-2221` (ref-state apply),
//! `sut.rs:1323-1325` (SUT apply), and
//! `transition_budgets.rs:137-143` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutViewControl;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;
use crate::pbt::validation::Reason;
use crate::pbt::validation::check;

/// Switch the current view (e.g. "all", "sidebar", "main").
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SwitchView {
    pub view_name: String,
}

impl TransitionFactory<ReferenceState> for SwitchView {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutViewControl,
        >()]
    }

    type Reason = Reason;
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

impl TransitionRef<ReferenceState> for SwitchView {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.action.app_started, Reason::AppNotStarted)];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.ui.user.current_view = self.view_name.clone();
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutViewControl> TransitionImpl<ReferenceState, S> for SwitchView {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.switch_view(&self.view_name).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for SwitchView {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

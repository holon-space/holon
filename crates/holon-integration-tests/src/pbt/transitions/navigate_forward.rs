//! Transition: navigate forward in the per-region navigation history.
//!
//! @pbt rung dispatch
//!   UNFAITHFUL SHORTCUT (audit TR-NAV): dispatches `navigation.go_forward`
//!   directly, bypassing the leader-chord path.
//! @pbt covers nav-forward — per-region navigation history forward (op-level
//! only)
//!
//! Mirrors the legacy logic split across `state_machine.rs:612-619`
//! (generator), `state_machine.rs:3171-3173` (precondition),
//! `state_machine.rs:2251-2259` (ref-state apply),
//! `sut.rs:1308-1314` (SUT apply), and
//! `transition_budgets.rs:176-181` (expected SQL).

use holon_api::Region;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefNavHistoryMut;
use holon_pbt_core::capabilities::SutNavHistoryDrive;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::JOURNAL_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::NAV_DML_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Pop one entry forward in the active navigation history for `region`.
/// Mirrors the forward-button in production's per-region history stack.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I navigate forward in region {region}")]
pub struct NavigateForward {
    pub region: Region,
}

impl<R: RefLifecycle + RefNavHistoryMut> TransitionFactory<R> for NavigateForward {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Restricted to Main — only TUI binding (leader+'f') targets
        // `region: "main"`. See `assets/default/keybindings.yaml`.
        let instance = NavigateForward {
            region: Region::Main,
        };
        instance.preconditions(state).map(|_| {
            let strat = proptest::strategy::Just(instance).boxed();
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefNavHistoryMut> TransitionRef<R> for NavigateForward {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.can_go_forward(self.region),
                Reason::NoNavigationHistory,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.nav_step_forward(self.region);
    }
}

crate::cap_transition! {
    NavigateForward: SutNavHistoryDrive,
    where R: [ RefLifecycle + RefNavHistoryMut ],
    |me, _state, sut| {
        sut.navigate_forward(me.region).await;
    }
    sql_budget: |_me, state| {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS - 2,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

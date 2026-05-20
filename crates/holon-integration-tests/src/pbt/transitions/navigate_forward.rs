//! Transition: navigate forward in the per-region navigation history.
//!
//! Mirrors the legacy logic split across `state_machine.rs:612-619` (generator),
//! `state_machine.rs:3171-3173` (precondition),
//! `state_machine.rs:2251-2259` (ref-state apply),
//! `sut.rs:1308-1314` (SUT apply), and
//! `transition_budgets.rs:176-181` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::Region;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Pop one entry forward in the active navigation history for `region`.
/// Mirrors the forward-button in production's per-region history stack.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NavigateForward {
    pub region: Region,
}

impl TransitionFactory<ReferenceState> for NavigateForward {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
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

impl TransitionRef<ReferenceState> for NavigateForward {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
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

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if let Some(history) = state.ui.tab.navigation_history.get_mut(&self.region)
            && history.cursor < history.entries.len() - 1
        {
            history.cursor += 1;
        }
        state.ui.tab.focused_entity_id.remove(&self.region);
        state.ui.tab.focused_cursor.remove(&self.region);

        // Blur on nav: see `navigate_focus.rs` for verification.
        state.blur_active_editor();
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for NavigateForward {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_navigate_forward(self.region).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for NavigateForward {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS - 2,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

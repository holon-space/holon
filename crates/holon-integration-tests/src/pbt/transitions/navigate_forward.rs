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

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Pop one entry forward in the active navigation history for `region`.
/// Mirrors the forward-button in production's per-region history stack.
#[derive(Clone, Debug)]
pub struct NavigateForward {
    pub region: Region,
}

impl E2ETransitionFactory for NavigateForward {
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

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for NavigateForward {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
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
        if let Some(history) = state.navigation_history.get_mut(&self.region)
            && history.cursor < history.entries.len() - 1
        {
            history.cursor += 1;
        }
        state.focused_entity_id.remove(&self.region);
        state.focused_cursor.remove(&self.region);

        // Blur on nav: see `navigate_focus.rs` for verification.
        state.active_editor = None;
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_navigate_forward(self.region).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS - 2,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

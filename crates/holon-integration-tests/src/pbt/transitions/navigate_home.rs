//! Transition: navigate home (return to root) in a region.
//!
//! Mirrors the legacy logic split across `state_machine.rs:621-626` (generator),
//! `state_machine.rs:3174` (precondition),
//! `state_machine.rs:2260-2276` (ref-state apply),
//! `sut.rs:1316-1330` (SUT apply), and
//! `transition_budgets.rs:182-187` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::Region;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::NavigationHistory;
use crate::pbt::reference_state::OpenPinEntry;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Return to root (home) in a region's navigation history.
/// Clears all navigation state for the region and sets focus to None globally.
#[derive(Clone, Debug)]
pub struct NavigateHome {
    pub region: Region,
}

impl E2ETransitionFactory for NavigateHome {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Restricted to Main: the only TUI binding for `go_home` is
        // leader+'h' which always targets `region: "main"`. See
        // `assets/default/keybindings.yaml`. The previous
        // generator emitted all three regions and the SUT covered the
        // gap with a `execute_op + manual set_focus` shortcut — that's
        // item A3 in `frontends/tui/TODO.md` removes.
        let instance = NavigateHome {
            region: Region::Main,
        };
        instance.preconditions(state).map(|_| {
            let strat = proptest::strategy::Just(instance).boxed();
            (1, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for NavigateHome {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started, Reason::AppNotStarted)];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let history = state
            .navigation_history
            .entry(self.region)
            .or_insert_with(NavigationHistory::new);

        history.entries.truncate(history.cursor + 1);
        history.entries.push(None);
        history.cursor = history.entries.len() - 1;

        // `go_home` is `focus(region, None)`: close all open in region, then
        // insert a new open row with block_id NULL. The home row is kept in
        // `open_pins` so `next_history_id` aligns with SQLite's AUTOINCREMENT,
        // but it's filtered out of `expected_focus_root_ids` (None block_id).
        let history_id = state.next_history_id;
        state.next_history_id += 1;
        let added_ts_logical = state.next_pin_ts;
        state.next_pin_ts += 1;
        let pins = state.open_pins.entry(self.region).or_default();
        pins.clear();
        pins.push(OpenPinEntry {
            history_id,
            block_id: None,
            added_ts_logical,
        });

        state.focused_entity_id.remove(&self.region);
        state.focused_cursor.remove(&self.region);

        // Mirror production: `maybe_mirror_navigation_focus` clears
        // UiState.focused_block globally on "go_home", regardless of
        // which region triggered it. See reactive.rs:1824.
        state.focused_block = None;

        // Same blur-on-click rationale as `navigate_focus.rs`: clear
        // `active_editor` (verified) but not `block.content` (separate
        // assumption left to invariants).
        state.active_editor = None;
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_navigate_home(self.region).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

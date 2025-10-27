//! Transition: navigate home (return to root) in a region.
//!
//! Mirrors the legacy logic split across `state_machine.rs:621-626`
//! (generator), `state_machine.rs:3174` (precondition),
//! `state_machine.rs:2260-2276` (ref-state apply),
//! `sut.rs:1316-1330` (SUT apply), and
//! `transition_budgets.rs:182-187` (expected SQL).

use holon_api::Region;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::SutNavHistoryWrite;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::OpenPinEntry;
use crate::pbt::reference_state::ReferenceState;
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
use crate::pbt::validation::Reason;
use crate::pbt::validation::check;

/// Return to root (home) in a region's navigation history.
/// Clears all navigation state for the region and sets focus to None globally.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NavigateHome {
    pub region: Region,
}

impl TransitionFactory<ReferenceState> for NavigateHome {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutNavHistoryWrite,
        >()]
    }

    type Reason = Reason;
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

impl TransitionRef<ReferenceState> for NavigateHome {
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
        // Idempotent like `navigate_focus`'s same-target focus: when already home
        // (current focus is `None`), production's `focus(region, None)` writes NO new
        // `navigation_history` row and NO new `open_pins` row — confirmed by
        // `headless_go_home_idempotency_probe` (go_home×3 → a single NULL home row).
        // Pushing a duplicate `None` / bumping `next_history_id` on a repeat `go_home`
        // would let `NavigateBack` walk back through phantom home entries the SUT never
        // created — the `NavigateHome→NavigateBack` divergence. This is the exact
        // back-stack-accumulation hazard `navigate_focus.rs`'s `already_focused` guard
        // documents.
        let already_home = state.current_focus(self.region).is_none();
        if !already_home {
            let history = state
                .ui
                .tab
                .navigation_history
                .entry(self.region)
                .or_default();

            history.entries.truncate(history.cursor + 1);
            history.entries.push(None);
            history.cursor = history.entries.len() - 1;

            // `go_home` is `focus(region, None)`: close all open in region, then
            // insert a new open row with block_id NULL. The home row is kept in
            // `open_pins` so `next_history_id` aligns with SQLite's AUTOINCREMENT,
            // but it's filtered out of `expected_focus_root_ids` (None block_id).
            let history_id = state.ui.tab.next_history_id;
            state.ui.tab.next_history_id += 1;
            let added_ts_logical = state.ui.user.next_pin_ts;
            state.ui.user.next_pin_ts += 1;
            let pins = state.ui.user.open_pins.entry(self.region).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: None,
                added_ts_logical,
            });
        }

        state.ui.tab.focused_entity_id.remove(&self.region);
        state.ui.tab.focused_cursor.remove(&self.region);

        // Mirror production: `maybe_mirror_navigation_focus` clears
        // UiState.focused_block globally on "go_home", regardless of
        // which region triggered it. See reactive.rs:1824.
        state.ui.tab.focused_block = None;

        // Same blur-on-click rationale as `navigate_focus.rs`: clear
        // `active_editor` (verified); `blur_active_editor` commits the pending
        // text only under a real editor (matching prod's `on_blur`).
        state.blur_active_editor();
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutNavHistoryWrite> TransitionImpl<ReferenceState, S> for NavigateHome {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        // The generator only ever emits `Region::Main`; map it to the cap's
        // `CapRegion`. Any other region is a generator bug, not a runtime case.
        let region = match self.region {
            Region::Main => CapRegion::Main,
            other => panic!("NavigateHome generator must only emit Main; got {other:?}"),
        };
        sut.apply_navigate_home(region).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for NavigateHome {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

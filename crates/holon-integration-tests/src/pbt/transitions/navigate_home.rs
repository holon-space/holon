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

use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::{
    CapRegion, RefFocus, RefFocusMut, RefLifecycle, RefNavHistoryMut, SutNavHistoryWrite,
};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

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

fn navigate_home_preconditions<R: RefLifecycle>(state: &R) -> Validated<(), Reason> {
    let checks: Vec<Validated<(), Reason>> =
        vec![check(state.app_started(), Reason::AppNotStarted)];
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

fn navigate_home_apply_to_ref<R: RefFocus + RefFocusMut + RefNavHistoryMut>(
    region: Region,
    state: &mut R,
) {
    // Idempotent like `navigate_focus`'s same-target focus: when already home
    // (current focus is `None`), production's `focus(region, None)` writes NO new
    // `navigation_history` row and NO new `open_pins` row — confirmed by
    // `headless_go_home_idempotency_probe` (go_home×3 → a single NULL home row).
    // Pushing a duplicate `None` / bumping `next_history_id` on a repeat `go_home`
    // would let `NavigateBack` walk back through phantom home entries the SUT never
    // created — the `NavigateHome→NavigateBack` divergence. This is the exact
    // back-stack-accumulation hazard `navigate_focus.rs`'s `already_focused` guard
    // documents.
    let already_home = state.current_focus(region_to_cap(region)).is_none();
    if !already_home {
        // `go_home` is `focus(region, None)`: push a NULL home entry, close all
        // open rows in the region, then insert a new open row with block_id NULL.
        // The home row is kept in `open_pins` so `next_history_id` aligns with
        // SQLite's AUTOINCREMENT, but it's filtered out of `expected_focus_root_ids`.
        state.nav_focus_push(region, None);
    }

    state.clear_region_focus(region);

    // Mirror production: `maybe_mirror_navigation_focus` clears
    // UiState.focused_block globally on "go_home", regardless of
    // which region triggered it. See reactive.rs:1824.
    state.set_global_focus(None);

    // Same blur-on-click rationale as `navigate_focus.rs`: clear
    // `active_editor` (verified); `blur_active_editor` commits the pending
    // text only under a real editor (matching prod's `on_blur`).
    state.blur_active_editor();
}

fn region_to_cap(region: Region) -> CapRegion {
    match region {
        Region::Main => CapRegion::Main,
        Region::LeftSidebar | Region::RightSidebar => CapRegion::Sidebar,
    }
}

impl TransitionRef<ReferenceState> for NavigateHome {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        navigate_home_preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        navigate_home_apply_to_ref(self.region, state);
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

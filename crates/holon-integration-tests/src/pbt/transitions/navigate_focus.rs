//! Transition: navigate to a specific block within a region.
//!
//! @pbt rung input-pipeline
//!   `apply_navigate_focus_via`: sidebar click_entity(left_sidebar) -> the
//!   entry's bound navigation.focus intent (find_click_intent -> dispatch).
//! @pbt covers sidebar-nav-focus — sidebar click -> navigation.focus intent +
//! SQL nav write
//!
//! Mirrors the legacy logic split across `state_machine.rs:568-601`
//! (generator), `state_machine.rs:3165-3167` (precondition),
//! `state_machine.rs:2222-2241` (ref-state apply),
//! `sut.rs:1266-1292` (SUT apply), and
//! `transition_budgets.rs:165-172` (expected SQL).

use holon_api::EntityUri;
use holon_api::Region;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefNavHistoryMut;
use holon_pbt_core::capabilities::SutFocusWrite;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

/// Canonical block id of the default LeftSidebar panel — the drawer whose
/// open/closed state gates whether sidebar page entries are clickable.
const LEFT_SIDEBAR_PANEL: &str = "block:default-left-sidebar";

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::FIRST_VISIT_VIEW_DDL;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::FIRST_VISIT_VIEW_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::JOURNAL_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::NAV_DML_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Navigate to focus on a specific block within a region.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NavigateFocus {
    pub region: Region,
    pub block_id: EntityUri,
}

impl<R: RefLifecycle + RefBlockTree + RefLayout + RefFocusRoots + RefNavHistoryMut>
    TransitionFactory<R> for NavigateFocus
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: this navigates by clicking a LeftSidebar page entry and
        // verifies the move through the `current_focus` SQL view (+ CDC drain).
        // The sidebar page list and `current_focus` projection are Turso-native;
        // a no-Turso session has neither, so gate it out of {Loro} slices.
        // (The in-memory NavigationProvider covers ClickBlock / ArrowNavigate /
        // NavigateBack / NavigateForward / NavigateHome / ToggleDrawer.)
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Restricted to Main: in production the only UI that triggers
        // `navigation.focus` is the LeftSidebar selectable's bound
        // action, and it ALWAYS targets `region: "main"`. The ref model
        // predicts the set: page blocks with non-special titles
        // (`predicted_sidebar_navigation_targets`) — same set the default
        // sidebar PRQL renders and the layout binds nav-focus on.
        let candidates: Vec<EntityUri> = state
            .predicted_sidebar_navigation_targets()
            .into_iter()
            .filter(|uri| {
                NavigateFocus {
                    region: Region::Main,
                    block_id: uri.clone(),
                }
                .preconditions(state)
                .is_good()
            })
            .collect();
        check(!candidates.is_empty(), Reason::SidebarFocusNotRendered).map(|_| {
            // Boost weight when Main's current focus has no text descendants
            // available to edit. Without this, `FocusEditableText` (and every
            // other transition gated on `main_editable_descendants` /
            // `focusable_rendered_block_ids(Main)`) is unreachable because
            // `StartApp` seeds focus on `block:journals`, which is initially
            // empty. The base weight of 3 means a 50-step random walk often
            // skips `NavigateFocus` entirely (verified seeds 1 and 7 both
            // produced 0 click/edit transitions), so the click-to-focus
            // pipeline never gets exercised. Mirrors the empty-doc weight
            // bump in `bulk_external_add.rs:65`. Once a text descendant
            // exists, the weight drops back to base so the rest of the
            // random strategy can run.
            let main_focus_roots = state.expected_focus_root_ids(CapRegion::Main);
            let main_has_text_descendant = state.all_block_ids().iter().any(|id| {
                state.is_text_block(id)
                    && !state.is_page_block(id)
                    && !state.is_layout_block(id)
                    && state.is_descendant_of_any(id, &main_focus_roots)
            });
            let weight = if main_has_text_descendant { 3 } else { 100 };

            let strat = prop::sample::select(candidates)
                .prop_map(|block_id| NavigateFocus {
                    region: Region::Main,
                    block_id,
                })
                .boxed();
            (weight, strat)
        })
    }
}

impl<R: RefLifecycle + RefBlockTree + RefNavHistoryMut> TransitionRef<R> for NavigateFocus {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                self.block_id.scheme() == "block",
                Reason::FocusedBlockMissing,
            ),
        ];

        // Production binds `navigation.focus(region=main)` only on the
        // default sidebar's rendered doc list — `predicts_navigation_focus`
        // is the pure-ref-state predicate for that set. Without this
        // gate the transition could fire on a sidebar entity prod treats
        // as a plain editor-focus click and `apply_to_ref` would push
        // a navigation-history entry prod never produced.
        checks.push(check(
            state.predicts_navigation_focus(&self.block_id, Region::LeftSidebar),
            Reason::SidebarFocusNotRendered,
        ));

        // A sidebar click-to-focus only works when the LeftSidebar drawer is
        // OPEN. When collapsed (via `ToggleDrawer`), production `columns.rs`
        // drops the panel from the layout and keeps only the toggle widget —
        // so the page entry is never rendered/laid-out and cannot be clicked.
        // Without this gate the SUT click falls through to a plain `set_focus`
        // (focus only, no nav-history write) while the ref records a focus move
        // → divergence that surfaces ~1000 lines later. A real user would re-open the
        // drawer first; that's a separate `ToggleDrawer` transition.
        checks.push(check(
            state.drawer_is_open(LEFT_SIDEBAR_PANEL),
            Reason::LeftSidebarDrawerClosed,
        ));

        // Block must be text-typed and not a layout block
        let block_exists = state.block_content(&self.block_id).is_some();
        checks.push(check(block_exists, Reason::FocusedBlockMissing));
        if block_exists {
            checks.push(check(
                state.is_text_block(&self.block_id),
                Reason::FocusedNotText,
            ));
        }
        checks.push(check(
            !state.is_layout_block(&self.block_id),
            Reason::FocusedInLayoutBlocks,
        ));

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole `focus(region, block_id)` reference effect — idempotency guard,
        // first-visit budget flag, history-row push, open-pin reset, region-focus
        // clear, global-focus set, editor blur — lives in
        // `RefNavHistoryMut::nav_focus` (mirrors `provider.rs::focus`).
        state.nav_focus(self.region, &self.block_id);
    }
}

crate::cap_transition! {
    NavigateFocus: SutFocusWrite,
    where R: [ RefLifecycle + RefBlockTree + RefNavHistoryMut ],
    |me, _state, sut| {
        // The generator only ever emits `Region::Main` (sidebar nav-focus binds
        // `region: "main"`); map it to the cap's `CapRegion`. Any other region is
        // a generator bug, not a runtime case to handle.
        let region = match me.region {
            Region::Main => CapRegion::Main,
            other => panic!("NavigateFocus generator must only emit Main; got {other:?}"),
        };
        sut.apply_navigate_focus(region, &me.block_id).await;
    }
    sql_budget: |_me, state| {
        // First navigation to a root creates its watch matviews (see
        // FIRST_VISIT_VIEW_READS); revisits reuse the known-views cache.
        let (first_visit_reads, first_visit_ddl) = if state.last_navigate_first_visit() {
            (FIRST_VISIT_VIEW_READS, FIRST_VISIT_VIEW_DDL)
        } else {
            (0, 0)
        };
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + first_visit_reads,
            writes: 0,
            ddl: first_visit_ddl,
            tolerance: docs_tolerance(state),
        }
    }
}

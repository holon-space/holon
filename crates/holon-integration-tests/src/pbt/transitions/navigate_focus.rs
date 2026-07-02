//! Transition: navigate to a specific block within a region.
//!
//! Mirrors the legacy logic split across `state_machine.rs:568-601` (generator),
//! `state_machine.rs:3165-3167` (precondition),
//! `state_machine.rs:2222-2241` (ref-state apply),
//! `sut.rs:1266-1292` (SUT apply), and
//! `transition_budgets.rs:165-172` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::Region;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_capabilities::RefModelPredict;
use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::{
    CapRegion, RefBlockTree, RefFocus, RefFocusMut, RefLifecycle, RefNavHistoryMut, RefToggles,
    SutFocusWrite,
};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

/// Canonical block id of the default LeftSidebar panel — the drawer whose
/// open/closed state gates whether sidebar page entries are clickable.
const LEFT_SIDEBAR_PANEL: &str = "block:default-left-sidebar";

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, FIRST_VISIT_VIEW_DDL, FIRST_VISIT_VIEW_READS, JOURNAL_READS, NAV_DML_READS,
    REACTIVE_BASE, docs_tolerance,
};

/// Navigate to focus on a specific block within a region.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NavigateFocus {
    pub region: Region,
    pub block_id: EntityUri,
}

impl TransitionFactory<ReferenceState> for NavigateFocus {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutFocusWrite,
        >()]
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
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
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
            let main_focus_roots = state.expected_focus_root_ids(Region::Main);
            let main_has_text_descendant = state.domain.block_state.blocks.iter().any(|(id, b)| {
                b.content_type == ContentType::Text
                    && !b.is_page()
                    && !state.domain.layout_blocks.contains(id)
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

fn navigate_focus_preconditions<R: RefLifecycle + RefModelPredict + RefToggles + RefBlockTree>(
    block_id: &EntityUri,
    state: &R,
) -> Validated<(), Reason> {
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(block_id.scheme() == "block", Reason::FocusedBlockMissing),
    ];

    // Production binds `navigation.focus(region=main)` only on the
    // default sidebar's rendered doc list — `predicts_navigation_focus`
    // is the pure-ref-state predicate for that set. Without this
    // gate the transition could fire on a sidebar entity prod treats
    // as a plain editor-focus click and `apply_to_ref` would push
    // a navigation-history entry prod never produced.
    checks.push(check(
        state.predicts_navigation_focus(block_id, Region::LeftSidebar),
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
        state.is_drawer_open(LEFT_SIDEBAR_PANEL),
        Reason::LeftSidebarDrawerClosed,
    ));

    // Block must be text-typed and not a layout block
    let exists = state.block_exists(block_id);
    checks.push(check(exists, Reason::FocusedBlockMissing));
    if exists {
        checks.push(check(state.is_text_block(block_id), Reason::FocusedNotText));
    }
    checks.push(check(
        !state.is_layout_block(block_id),
        Reason::FocusedInLayoutBlocks,
    ));

    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

fn navigate_focus_apply_to_ref<R: RefFocus + RefFocusMut + RefNavHistoryMut>(
    region: Region,
    block_id: EntityUri,
    state: &mut R,
) {
    // Re-focusing the block that is already this region's current focus is
    // idempotent in prod: `navigation.focus` on the active target writes no
    // new `navigation_history` row (the SUT stays at exactly one row, id and
    // cursor unmoved — confirmed across ~15 consecutive same-block focuses).
    // Pushing a new back-stack entry / bumping `next_history_id` here would
    // let the ref accumulate duplicate entries and walk back through them on
    // NavigateBack while prod goes to home — an inv-navigation-focus divergence.
    let already_focused = state.current_focus(region_to_cap(region)).as_ref() == Some(&block_id);

    // Budget model: the first navigation to a root renders it for the
    // first time and creates its watch matviews; `expected_sql` grants
    // the creation allowance off this flag (recorded pre-insert because
    // the budget invariant only sees the post-apply state).
    state.mark_navigation_visit(&block_id);

    if !already_focused {
        // Mirror provider.rs `focus`: push a back-stack entry, close all open
        // rows in the region, then insert a new open row. `next_history_id`
        // matches SQLite's AUTOINCREMENT, monotonic across INSERTs and
        // unaffected by UPDATE (closed_at flip) or DELETE. A same-block
        // re-focus inserts no row, so the counter must not advance either.
        state.nav_focus_push(region, Some(block_id.clone()));
    }

    // NavigateFocus changes what's displayed but clears editor focus —
    // the previously-focused block may no longer be visible.
    state.clear_region_focus(region);

    // Mirror `UiState::set_focus`: the navigation target becomes the
    // globally focused block. `focus_chain()` and `chain_ops()` read
    // from this — inv-value-fn-provider-arg-variance asserts they reflect the predicted URI.
    state.set_global_focus(Some(block_id));

    // Production blurs the editor on this click — empirically verified
    // by seed 8 of the post-Blur-deletion PBT run, where leaving
    // `active_editor` set let TypeChars fire and panic with
    // "GPUI keystroke not consumed: keystroke=\"d\"" (devlog
    // 2026-05-08-133241). `blur_active_editor` commits the pending text
    // on blur ONLY under a real editor (`real_editor_enabled`), mirroring
    // prod's `on_blur` → `set_field("content")`; the headless slices (no
    // real editor) keep the prior "don't pre-bake the commit" behaviour.
    state.blur_active_editor();
}

fn region_to_cap(region: Region) -> CapRegion {
    match region {
        Region::Main => CapRegion::Main,
        Region::LeftSidebar | Region::RightSidebar => CapRegion::Sidebar,
    }
}

impl TransitionRef<ReferenceState> for NavigateFocus {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        navigate_focus_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        navigate_focus_apply_to_ref(self.region, self.block_id.clone(), state);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutFocusWrite> TransitionImpl<ReferenceState, S> for NavigateFocus {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        // The generator only ever emits `Region::Main` (sidebar nav-focus binds
        // `region: "main"`); map it to the cap's `CapRegion`. Any other region is
        // a generator bug, not a runtime case to handle.
        let region = match self.region {
            Region::Main => CapRegion::Main,
            other => panic!("NavigateFocus generator must only emit Main; got {other:?}"),
        };
        sut.apply_navigate_focus(region, &self.block_id).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for NavigateFocus {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        // First navigation to a root creates its watch matviews (see
        // FIRST_VISIT_VIEW_READS); revisits reuse the known-views cache.
        let (first_visit_reads, first_visit_ddl) = if state.ui.tab.last_navigate_first_visit {
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

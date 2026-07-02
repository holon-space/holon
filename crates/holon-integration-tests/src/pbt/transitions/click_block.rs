//! Transition: click on a focusable rendered block to focus it.
//!
//! Mirrors the legacy logic split across `state_machine.rs:628-670` (generator),
//! `state_machine.rs:3175-3180` (precondition),
//! `state_machine.rs:2277-2315` (ref-state apply),
//! `sut.rs:2282-2394` (SUT apply), and
//! `transition_budgets.rs:190-196` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_pbt_core::capabilities::{SutDriver, SutLayout};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use std::time::Duration;
use validated::Validated;

use crate::pbt::reference_capabilities::RefModelPredict;
use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::{
    CapCursor, RefBlockTree, RefEditorMirror, RefFocusMut, RefLifecycle, RefNavHistoryMut,
    SutBlockInteract,
};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

use holon_api::{EntityUri, Region};

// ── Capability-bound free function (Phase C) ──────────────────────

/// SUT-side body of `ClickBlock`. Bound on `SutLayout + SutDriver` so any
/// slice supplying both capabilities can include this transition.
///
/// Three-step protocol — and only three steps. The driver's
/// `click_entity` impl already encodes the medium difference: GPUI
/// dispatches a real MouseDown; ReactiveEngineDriver polls
/// `snapshot_resolved`, looks up the bound click intent via
/// `find_click_intent_in_region`, and either applies it or falls back
/// to a plain `set_focus` (focus is in-memory state, ADR 0010). The
/// transition layer trusts that contract and stays medium-agnostic.
///
/// 1. `wait_for_bounds` — GPUI's `click_entity` reads BoundsRegistry, so
///    the target must be registered first. Headless: no-op.
/// 2. `click_entity` — unified dispatch.
/// 3. `wait_for_engine_focus` — GPUI's dispatch_intent is
///    fire-and-forget; the focus mirror needs an explicit barrier
///    before subsequent transitions read it.
pub async fn apply_click_block_to_sut<S: SutLayout + SutDriver>(
    sut: &S,
    region: &str,
    id: &EntityUri,
) {
    sut.wait_for_bounds(id, Duration::from_secs(5))
        .await
        .unwrap_or_else(|e| panic!("[ClickBlock] bounds unavailable for {id}: {e}"));
    sut.click_entity(id, region)
        .await
        .unwrap_or_else(|e| panic!("[ClickBlock] click_entity failed for {id}: {e}"));
    sut.wait_for_engine_focus(id, Duration::from_secs(2))
        .await
        .unwrap_or_else(|e| panic!("[ClickBlock] focus did not propagate within 2s for {id}: {e}"));
}

/// Click on a rendered block to focus it. When clicking in LeftSidebar,
/// also pushes a navigation-history entry for Region::Main.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ClickBlock {
    pub region: Region,
    pub block_id: EntityUri,
}

impl TransitionFactory<ReferenceState> for ClickBlock {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutBlockInteract,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let main_unfocused = state.current_focus(Region::Main).is_none();
        let mut arms: Vec<(u32, BoxedStrategy<ClickBlock>)> = Vec::new();

        // LeftSidebar candidates: the ref-predicted set of sidebar entities
        // that production wraps in `selectable(navigation.focus(region=main))`.
        // The default sidebar PRQL renders page blocks with non-special
        // titles; the layout binds nav-focus on each row. Filtering here
        // mirrors what the previous driver-based path (`entities_in_region`
        // + click_intent_of) computed, but from pure ref state.
        let sidebar_candidates: Vec<EntityUri> = state.predicted_sidebar_navigation_targets();
        if !sidebar_candidates.is_empty() {
            let weight = if main_unfocused { 12 } else { 3 };
            arms.push((
                weight,
                proptest::sample::select(sidebar_candidates)
                    .prop_map(|block_id| ClickBlock {
                        region: Region::LeftSidebar,
                        block_id,
                    })
                    .boxed(),
            ));
        }

        // Main candidates: text blocks the user can edit (no bound click
        // action — clicking places the editor cursor). `main_editable_descendants`
        // already encodes content_type / non-page / non-layout / focusable
        // / non-locked, which lines up with preconditions exactly.
        let main_candidates: Vec<EntityUri> = state.main_editable_descendants();
        if !main_candidates.is_empty() {
            arms.push((
                3,
                proptest::sample::select(main_candidates)
                    .prop_map(|block_id| ClickBlock {
                        region: Region::Main,
                        block_id,
                    })
                    .boxed(),
            ));
        }

        // RightSidebar still skipped — its default PRQL depends on a focus
        // the PBT's nav-state doesn't mirror in the production matview
        // chain, so clicks time out waiting for content that never
        // resolves. Re-enable once that's untangled.

        check(!arms.is_empty(), Reason::NoFocusableBlocks).map(|_| {
            let strat = proptest::strategy::Union::new_weighted(arms).boxed();
            (1, strat)
        })
    }
}

fn click_block_preconditions<R: RefLifecycle + RefBlockTree>(
    block_id: &EntityUri,
    state: &R,
) -> Validated<(), Reason> {
    // Visibility / rendered-set membership is no longer a precondition.
    // The driver's wait-for-bounds with scroll-into-view (sut.rs)
    // covers "must be reachable on screen"; a real bug surfaces as
    // the wait timeout, not as a precondition rejection. `is_focusable`
    // / `!is_page` / `!layout_blocks` stay ref-state model facts.
    let exists = state.block_exists(block_id);
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(exists, Reason::FocusedBlockMissing),
    ];
    if exists {
        checks.push(check(state.is_text_block(block_id), Reason::FocusedNotText));
    }
    checks.push(check(
        !state.is_layout_block(block_id),
        Reason::FocusedInLayoutBlocks,
    ));
    checks.push(check(
        state.is_focusable(block_id),
        Reason::FocusedNotFocusable,
    ));
    checks.push(check(
        exists && !state.is_page_block(block_id),
        Reason::FocusedIsPage,
    ));
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

fn click_block_apply_to_ref<
    R: RefEditorMirror + RefModelPredict + RefFocusMut + RefNavHistoryMut,
>(
    region: Region,
    block_id: &EntityUri,
    state: &mut R,
) {
    // A real click anywhere outside the active editor blurs it, and the
    // editor's `on_blur` → `set_field("content")` commits pending text
    // (real-editor runs only — `blur_active_editor` carries the gate).
    // Same-block clicks don't blur (the driver skips click-when-focused),
    // so leave those untouched.
    if state.active_editor_block().is_some_and(|b| &b != block_id) {
        state.blur_active_editor();
    }
    // The default LeftSidebar wraps each doc in a `selectable` whose
    // bound action is `navigation.focus(region: "main", block_id: col("id"))`.
    // Clicking it dispatches that intent, which the production
    // navigation provider maps to a navigation-history push for
    // region=Main. Mirror that here so `focus_roots` / `current_focus`
    // checks line up with the real backend after the click.
    //
    // Other regions (Main, RightSidebar), AND sidebar clicks on
    // entities the default sidebar PRQL does NOT render (so prod
    // has no selectable bound), fall through to editor focus.
    if state.predicts_navigation_focus(block_id, region) {
        // Same close-then-insert as NavigateFocus — see navigate_focus.rs
        // for rationale. The sidebar selectable always targets region=Main.
        state.nav_focus_push(Region::Main, Some(block_id.clone()));
        state.clear_region_focus(Region::Main);
        state.set_global_focus(Some(block_id.clone()));
    } else {
        // Clicking sets editor focus but does NOT change the navigation cursor.
        // The user is still viewing the same document; only the focused editor
        // changes. Arrow keys will now navigate among the clicked block's siblings.
        // The global `focused_block` mirror also follows the click — production
        // GPUI's `render_entity` / `rendered_text` click handlers call
        // `services.set_focus(Some(id))` directly (focus is in-memory state,
        // ADR 0010; no `editor_focus` dispatch).
        state.set_global_focus(Some(block_id.clone()));
        state.set_region_focus(region, block_id.clone(), CapCursor::default());
    }
}

impl TransitionRef<ReferenceState> for ClickBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        click_block_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        click_block_apply_to_ref(self.region, &self.block_id, state);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutBlockInteract> TransitionImpl<ReferenceState, S> for ClickBlock {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.click_block(self.region, &self.block_id).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ClickBlock {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5,
        }
    }
}

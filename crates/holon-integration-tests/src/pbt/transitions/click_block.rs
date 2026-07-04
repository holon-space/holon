//! Transition: click on a focusable rendered block to focus it.
//!
//! @pbt rung input-pipeline
//!   `apply_click_block_to_sut`: wait_for_bounds + click_entity +
//!   wait_for_engine_focus through the production UserDriver.
//! @pbt covers click-to-focus — pointer click -> find_click_intent -> focus
//!
//! Mirrors the legacy logic split across `state_machine.rs:628-670`
//! (generator), `state_machine.rs:3175-3180` (precondition),
//! `state_machine.rs:2277-2315` (ref-state apply),
//! `sut.rs:2282-2394` (SUT apply), and
//! `transition_budgets.rs:190-196` (expected SQL).

use std::time::Duration;

use holon_api::EntityUri;
use holon_api::Region;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefNavHistory;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::capabilities::SutDriver;
use holon_pbt_core::capabilities::SutLayout;
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
/// 1. `wait_for_bounds` — GPUI's `click_entity` reads BoundsRegistry, so the
///    target must be registered first. Headless: no-op.
/// 2. `click_entity` — unified dispatch.
/// 3. `wait_for_engine_focus` — GPUI's dispatch_intent is fire-and-forget; the
///    focus mirror needs an explicit barrier before subsequent transitions read
///    it.
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

impl<R: RefLifecycle + RefBlockTree + RefFocus + RefNavHistory + RefLayoutMutate>
    TransitionFactory<R> for ClickBlock
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let main_unfocused = state.current_focus(CapRegion::Main).is_none();
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
                super::select_bias::select_with_edge_bias(main_candidates)
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

impl<R: RefLifecycle + RefBlockTree + RefFocus + RefNavHistory + RefLayoutMutate> TransitionRef<R>
    for ClickBlock
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        // Visibility / rendered-set membership is no longer a precondition.
        // The driver's wait-for-bounds with scroll-into-view (sut.rs)
        // covers "must be reachable on screen"; a real bug surfaces as
        // the wait timeout, not as a precondition rejection. `is_focusable`
        // / `!is_page` / `!layout_blocks` stay ref-state model facts.
        let block_exists = state.block_content(&self.block_id).is_some();
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(block_exists, Reason::FocusedBlockMissing),
        ];
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
        checks.push(check(
            state.is_focusable(&self.block_id),
            Reason::FocusedNotFocusable,
        ));
        checks.push(check(
            block_exists && !state.is_page_block(&self.block_id),
            Reason::FocusedIsPage,
        ));
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole click-focus reference effect (blur-other-editor, sidebar
        // nav-focus push vs. editor focus) lives in
        // `RefLayoutMutate::apply_click_focus`.
        state.apply_click_focus(self.region, &self.block_id);
    }
}

crate::cap_transition! {
    ClickBlock: SutBlockInteract,
    where R: [ RefLifecycle + RefBlockTree + RefFocus + RefNavHistory + RefLayoutMutate ],
    |me, _state, sut| {
        sut.click_block(me.region, &me.block_id).await;
    }
    sql_budget: |_me, state| {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5,
        }
    }
}

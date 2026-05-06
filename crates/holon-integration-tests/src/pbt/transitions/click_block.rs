//! Transition: click on a focusable rendered block to focus it.
//!
//! Mirrors the legacy logic split across `state_machine.rs:628-670` (generator),
//! `state_machine.rs:3175-3180` (precondition),
//! `state_machine.rs:2277-2315` (ref-state apply),
//! `sut.rs:2282-2394` (SUT apply), and
//! `transition_budgets.rs:190-196` (expected SQL).

use crate::pbt::validation::{Reason, check};
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

use holon_api::{ContentType, EntityUri, Region};

/// Click on a rendered block to focus it. When clicking in LeftSidebar,
/// also pushes a navigation-history entry for Region::Main.
#[derive(Clone, Debug)]
pub struct ClickBlock {
    pub region: Region,
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for ClickBlock {
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

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for ClickBlock {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        // Visibility / rendered-set membership is no longer a precondition.
        // The driver's wait-for-bounds with scroll-into-view (sut.rs)
        // covers "must be reachable on screen"; a real bug surfaces as
        // the wait timeout, not as a precondition rejection. `is_focusable`
        // / `!is_page` / `!layout_blocks` stay ref-state model facts.
        let block = state.block_state.blocks.get(&self.block_id);
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(block.is_some(), Reason::FocusedBlockMissing),
        ];
        if let Some(b) = block {
            checks.push(check(
                b.content_type == ContentType::Text,
                Reason::FocusedNotText,
            ));
        }
        checks.push(check(
            !state.layout_blocks.contains(&self.block_id),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            state.layout_blocks.is_focusable(&self.block_id),
            Reason::FocusedNotFocusable,
        ));
        checks.push(check(
            block.is_some_and(|b| !b.is_page()),
            Reason::FocusedIsPage,
        ));
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        use crate::pbt::reference_state::NavigationHistory;
        use crate::pbt::reference_state::OpenPinEntry;
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
        if state.predicts_navigation_focus(&self.block_id, self.region) {
            let history = state
                .navigation_history
                .entry(Region::Main)
                .or_insert_with(NavigationHistory::new);
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(self.block_id.clone()));
            history.cursor = history.entries.len() - 1;

            // Same close-then-insert as NavigateFocus — see navigate_focus.rs
            // for rationale.
            let history_id = state.next_history_id;
            state.next_history_id += 1;
            let added_ts_logical = state.next_pin_ts;
            state.next_pin_ts += 1;
            let pins = state.open_pins.entry(Region::Main).or_default();
            pins.clear();
            pins.push(OpenPinEntry {
                history_id,
                block_id: Some(self.block_id.clone()),
                added_ts_logical,
            });

            state.focused_entity_id.remove(&Region::Main);
            state.focused_cursor.remove(&Region::Main);
            state.focused_block = Some(self.block_id.clone());
        } else {
            // Clicking sets editor focus but does NOT change the navigation cursor.
            // The user is still viewing the same document; only the focused editor
            // changes. Arrow keys will now navigate among the clicked block's siblings.
            // The global `focused_block` mirror also follows the click — production
            // GPUI's `render_entity` click handler calls `services.set_focus(Some(id))`
            // before dispatching `editor_focus`.
            state.focused_block = Some(self.block_id.clone());
            state
                .focused_entity_id
                .insert(self.region, self.block_id.clone());
            state.focused_cursor.insert(
                self.region,
                crate::pbt::reference_state::CursorPosition::start(),
            );
        }
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_click_block(self.region, &self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5,
        }
    }
}

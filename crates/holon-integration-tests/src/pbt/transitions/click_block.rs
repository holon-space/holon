//! Transition: click on a focusable rendered block to focus it.
//!
//! Mirrors the legacy logic split across `state_machine.rs:628-670` (generator),
//! `state_machine.rs:3175-3180` (precondition),
//! `state_machine.rs:2277-2315` (ref-state apply),
//! `sut.rs:2282-2394` (SUT apply), and
//! `transition_budgets.rs:190-196` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

use holon_api::{EntityUri, Region};

/// Click on a rendered block to focus it. When clicking in LeftSidebar,
/// also pushes a navigation-history entry for Region::Main.
#[derive(Clone, Debug)]
pub struct ClickBlock {
    pub region: Region,
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for ClickBlock {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        let regions = Region::ALL.to_vec();
        let main_unfocused = state.current_focus(Region::Main).is_none();
        let mut arms: Vec<(u32, BoxedStrategy<ClickBlock>)> = Vec::new();
        for region in &regions {
            // Skip RightSidebar while we stabilize the bug reproduction —
            // its default PRQL is `from children`, which depends on a focus
            // that the PBT's nav-state doesn't fully mirror in the production
            // matview chain. Clicking ends up timing out waiting for content
            // that never resolves. Re-enable once we either teach the ref
            // model to seed RightSidebar focus correctly or extend the click
            // path to handle non-clickable targets gracefully.
            if *region == Region::RightSidebar {
                continue;
            }
            let focusable = state.focusable_rendered_block_ids(*region);
            if !focusable.is_empty() {
                let r = *region;
                let weight = if main_unfocused && *region == Region::LeftSidebar {
                    12
                } else {
                    3
                };
                arms.push((
                    weight,
                    proptest::sample::select(focusable)
                        .prop_map(move |block_id| ClickBlock {
                            region: r,
                            block_id,
                        })
                        .boxed(),
                ));
            }
        }
        if arms.is_empty() {
            return None;
        }
        let strat = proptest::strategy::Union::new_weighted(arms).boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for ClickBlock {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started
            && state.block_state.blocks.contains_key(&self.block_id)
            && state.layout_blocks.is_focusable(&self.block_id)
            && !state.focusable_rendered_block_ids(self.region).is_empty()
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        use crate::pbt::reference_state::NavigationHistory;
        // The default LeftSidebar wraps each doc in a `selectable` whose
        // bound action is `navigation.focus(region: "main", block_id: col("id"))`.
        // Clicking it dispatches that intent, which the production
        // navigation provider maps to a navigation-history push for
        // region=Main. Mirror that here so `focus_roots` / `current_focus`
        // checks line up with the real backend after the click.
        //
        // Other regions (Main, RightSidebar) don't have bound actions
        // in the default layout — clicking just sets editor focus.
        if self.region == Region::LeftSidebar {
            let history = state
                .navigation_history
                .entry(Region::Main)
                .or_insert_with(NavigationHistory::new);
            history.entries.truncate(history.cursor + 1);
            history.entries.push(Some(self.block_id.clone()));
            history.cursor = history.entries.len() - 1;

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

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
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

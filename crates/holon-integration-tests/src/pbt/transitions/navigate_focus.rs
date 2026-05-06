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

use super::E2ETransitionImpl;
use crate::pbt::reference_state::NavigationHistory;
use crate::pbt::reference_state::OpenPinEntry;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Navigate to focus on a specific block within a region.
#[derive(Clone, Debug)]
pub struct NavigateFocus {
    pub region: Region,
    pub block_id: EntityUri,
}

impl E2ETransitionFactory for NavigateFocus {
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
            let main_has_text_descendant = state.block_state.blocks.iter().any(|(id, b)| {
                b.content_type == ContentType::Text
                    && !b.is_page()
                    && !state.layout_blocks.contains(id)
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

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for NavigateFocus {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let mut checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
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

        // Block must be text-typed and not a layout block
        let block = state.block_state.blocks.get(&self.block_id);
        checks.push(check(block.is_some(), Reason::FocusedBlockMissing));
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
        history.entries.push(Some(self.block_id.clone()));
        history.cursor = history.entries.len() - 1;

        // Mirror provider.rs `focus`: close all open rows in the region,
        // then insert a new open row. `next_history_id` matches SQLite's
        // AUTOINCREMENT, which monotonically increases across INSERTs and
        // is unaffected by UPDATE (closed_at flip) or DELETE.
        let history_id = state.next_history_id;
        state.next_history_id += 1;
        let added_ts_logical = state.next_pin_ts;
        state.next_pin_ts += 1;
        let pins = state.open_pins.entry(self.region).or_default();
        pins.clear();
        pins.push(OpenPinEntry {
            history_id,
            block_id: Some(self.block_id.clone()),
            added_ts_logical,
        });

        // NavigateFocus changes what's displayed but clears editor focus —
        // the previously-focused block may no longer be visible.
        state.focused_entity_id.remove(&self.region);
        state.focused_cursor.remove(&self.region);

        // Mirror `UiState::set_focus`: the navigation target becomes the
        // globally focused block. `focus_chain()` and `chain_ops()` read
        // from this — inv-value-fn-provider-arg-variance asserts they reflect the predicted URI.
        state.focused_block = Some(self.block_id.clone());

        // Production blurs the editor on this click — empirically verified
        // by seed 8 of the post-Blur-deletion PBT run, where leaving
        // `active_editor` set let TypeChars fire and panic with
        // "GPUI keystroke not consumed: keystroke=\"d\"" (devlog
        // 2026-05-08-133241). We deliberately do NOT call
        // `commit_active_editor_if_changed()` — whether prod's `on_blur`
        // dispatches `set_field` is a separate assumption the PBT's
        // content invariants should verify, not pre-bake.
        state.active_editor = None;
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_navigate_focus(self.region, &self.block_id).await;
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

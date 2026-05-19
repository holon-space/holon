//! Transition: toggle the task state of a block via the StateToggle widget path.
//!
//! Mirrors the legacy logic split across `state_machine.rs:941-1054` (generator),
//! `state_machine.rs:3236-3262` (precondition),
//! `state_machine.rs:2519-2533` (ref-state apply),
//! `sut.rs:2176-2359` (SUT apply), and
//! `transition_budgets.rs:279-281` (expected SQL).

use holon_pbt_core::capabilities::{SutDriver, SutLayout};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use std::time::Duration;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

/// Production task-state cycle order, hardcoded in
/// `sql_operation_provider.rs:1525-1526` (the `cycle_task_state` op
/// implementation). Each click of the state_toggle widget advances by
/// one position; full cycle wraps back to `""`.
///
/// Kept in sync with the upstream constant — this lookup table is the
/// click-count semantics, not a separate model of behaviour.
pub const TASK_STATE_CYCLE: &[&str] = &["", "TODO", "DOING", "DONE"];

/// Compute how many state_toggle clicks advance the cycle from
/// `current` to `target`. Panics if either is outside the production
/// cycle — the generator's `RENDERED_DEFAULT_STATES` matches the
/// production order, so a mismatch means generator drift.
///
/// Returns a value in `1..=TASK_STATE_CYCLE.len()`; `0` is unreachable
/// because the generator excludes no-op transitions (`current ==
/// target`), and a same-state cycle would otherwise return 0 here.
/// The SutHandle adapter asserts `>0` defensively as a generator-drift
/// guard.
pub fn cycle_click_count(current: &str, target: &str) -> u8 {
    let cur_idx = TASK_STATE_CYCLE
        .iter()
        .position(|s| *s == current)
        .unwrap_or_else(|| {
            panic!(
                "[ToggleState] current state {current:?} not in production cycle {TASK_STATE_CYCLE:?}"
            )
        });
    let tgt_idx = TASK_STATE_CYCLE
        .iter()
        .position(|s| *s == target)
        .unwrap_or_else(|| {
            panic!(
                "[ToggleState] target state {target:?} not in production cycle {TASK_STATE_CYCLE:?}"
            )
        });
    ((tgt_idx + TASK_STATE_CYCLE.len() - cur_idx) % TASK_STATE_CYCLE.len()) as u8
}

// ── Capability-bound free function (Phase C, Option A — real user input) ──
//
// Replaces the previous body's apply_intent backend dispatch with N real
// clicks on the state_toggle widget — exactly what a user would do.
// Each click fires the bound `cycle_task_state` op, advancing the cycle
// one step. Between clicks we yield twice so CDC propagates to the
// rendered `current` prop before the next cycle reads it.

/// SUT-side body of `ToggleState`. Bound on `SutLayout + SutDriver`.
/// Click the state_toggle widget `click_count` times to advance the
/// task_state cycle to the target.
pub async fn apply_toggle_state_to_sut<S: SutLayout + SutDriver>(
    sut: &mut S,
    id: &EntityUri,
    click_count: u8,
) {
    sut.wait_for_widget_kind(id, &["state_toggle"], Duration::from_secs(2))
        .await
        .unwrap_or_else(|e| panic!("[ToggleState] target {id} not rendered as state_toggle: {e}"));
    for n in 0..click_count {
        sut.click_entity(id, "main")
            .await
            .unwrap_or_else(|e| panic!("[ToggleState] click #{} failed for {id}: {e}", n + 1));
        // Let CDC propagate so the next click reads the post-cycle
        // `current` from the matview. Without this the cycle may
        // double-advance on the next click.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }
}

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use holon_api::EntityUri;
use holon_orgmode::OrgBlockExt;

/// Toggle the task state of a block via the StateToggle widget.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToggleState {
    pub block_id: EntityUri,
    pub new_state: String,
}

impl TransitionFactory<ReferenceState> for ToggleState {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let owned_render_expr = state
            .main_panel_render_expr()
            .or_else(|| state.root_render_expr())
            .cloned()
            .unwrap_or_else(super::super::reference_state::default_root_render_expr);

        let main_focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let visible_text_block_ids: Vec<EntityUri> = state
            .block_state
            .blocks
            .values()
            .filter(|b| {
                b.content_type == holon_api::ContentType::Text
                    && !b.is_page()
                    && !state.layout_blocks.contains(&b.id)
                    && main_focus_roots.contains(&b.id)
            })
            .map(|b| b.id.clone())
            .collect();

        let rows: Vec<holon_api::widget_spec::DataRow> = visible_text_block_ids
            .iter()
            .filter_map(|id| state.block_state.blocks.get(id))
            .map(super::super::reference_state::block_to_data_row)
            .collect();
        let arc_rows: Vec<std::sync::Arc<_>> = rows.into_iter().map(std::sync::Arc::new).collect();
        // Generator-side use: computes which blocks render a state_toggle so the
        // generator only proposes valid candidates. Not in the SUT application path
        // (which goes through SutDriver clicks).
        // ALLOW(pbt-sut-handle-frontend-simulation): generator-side render lookup
        let vm = holon_frontend::interpret_pure(&owned_render_expr, &arc_rows, state);
        let toggle_block_ids: Vec<EntityUri> = vm
            .snapshot()
            .state_toggle_block_ids()
            .into_iter()
            .filter_map(|id| holon_api::EntityUri::parse(&id).ok())
            .collect();

        const RENDERED_DEFAULT_STATES: [&str; 4] = ["", "TODO", "DOING", "DONE"];
        let candidate_states: Vec<String> = RENDERED_DEFAULT_STATES
            .iter()
            .map(|s| s.to_string())
            .collect();
        let pairs: Vec<(EntityUri, String)> = toggle_block_ids
            .iter()
            .filter(|id| {
                ToggleState {
                    block_id: (*id).clone(),
                    new_state: "".to_string(), // dummy for preconditions check
                }
                .preconditions(state)
                .is_good()
            })
            .flat_map(|id| {
                let current_state = state
                    .block_state
                    .blocks
                    .get(id)
                    .and_then(|b| b.task_state())
                    .map(|ts| ts.keyword.to_string())
                    .unwrap_or_default();
                let bid = id.clone();
                candidate_states
                    .iter()
                    .filter(move |&s| s != &current_state)
                    .cloned()
                    .map(move |s| (bid.clone(), s))
            })
            .collect();

        check(!pairs.is_empty(), Reason::NoTogglableStates).map(|_| {
            let strat = prop::sample::select(pairs)
                .prop_map(|(block_id, new_state)| ToggleState {
                    block_id,
                    new_state,
                })
                .boxed();
            (1, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for ToggleState {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(holon_api::Region::Main);
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(
                state.current_focus(holon_api::Region::Main).is_some(),
                Reason::NoFocusInMain,
            ),
            check(
                focus_roots.contains(&self.block_id),
                Reason::FocusedNotDescendantOfFocusRoot,
            ),
            // Layout headlines (in `layout_blocks.headline_ids`) define
            // their own render expression via a child render source.
            // Production renders the headline through that custom
            // layout, which can omit `state_toggle` entirely. The
            // headline never appears as a state_toggle entity in the
            // resolved ViewModel, so ToggleState would time out.
            // EditViaViewModel/Indent/MoveUp etc. already exclude
            // layout blocks for the same reason.
            check(
                !state.layout_blocks.contains(&self.block_id),
                Reason::FocusedInLayoutBlocks,
            ),
            // A custom entity profile for `block` can replace the
            // default render with anything (e.g. just an
            // `editable_text`) — losing the state_toggle widget.
            // The reference state doesn't introspect the active
            // variant's widget set, so conservatively skip
            // ToggleState whenever a custom block profile is loaded.
            check(
                !state.has_blocks_profile(),
                Reason::StateToggleNotApplicable,
            ),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.push_undo_snapshot();
        state.apply_mutation(&crate::pbt::types::MutationEvent {
            source: crate::pbt::types::MutationSource::UI,
            mutation: crate::pbt::types::Mutation::Update {
                entity: "block".to_string(),
                id: self.block_id.clone(),
                fields: [(
                    "task_state".to_string(),
                    holon_api::Value::String(self.new_state.clone()),
                )]
                .into(),
            },
        });
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for ToggleState {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_toggle_state(&self.block_id, &self.new_state)
            .await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ToggleState {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        expected_sql_for_kind(MutationKind::Update, watches, blocks, docs)
    }
}

#[cfg(test)]
mod cycle_click_count_tests {
    use super::{TASK_STATE_CYCLE, cycle_click_count};

    #[test]
    fn cycle_order_matches_production() {
        // Locked to sql_operation_provider.rs:1525-1526. If production
        // changes, this test breaks loudly and we update both sides.
        assert_eq!(TASK_STATE_CYCLE, &["", "TODO", "DOING", "DONE"]);
    }

    #[test]
    fn single_step_clicks() {
        assert_eq!(cycle_click_count("", "TODO"), 1);
        assert_eq!(cycle_click_count("TODO", "DOING"), 1);
        assert_eq!(cycle_click_count("DOING", "DONE"), 1);
        assert_eq!(cycle_click_count("DONE", ""), 1);
    }

    #[test]
    fn multi_step_clicks() {
        assert_eq!(cycle_click_count("", "DOING"), 2);
        assert_eq!(cycle_click_count("", "DONE"), 3);
        assert_eq!(cycle_click_count("TODO", "DONE"), 2);
    }

    #[test]
    fn wraps_around_cycle() {
        // DONE → "" → TODO is 2 clicks (wraps through empty).
        assert_eq!(cycle_click_count("DONE", "TODO"), 2);
        assert_eq!(cycle_click_count("DONE", "DOING"), 3);
        assert_eq!(cycle_click_count("DOING", "TODO"), 3);
    }

    #[test]
    fn same_state_returns_full_cycle_length() {
        // SutHandle adapter asserts > 0 to catch the no-op transition
        // case; the generator already excludes these. This test pins
        // the modular-arithmetic edge case so a refactor that breaks
        // it fails loudly.
        assert_eq!(
            cycle_click_count("TODO", "TODO"),
            0,
            "same-state click_count is 0 (full cycle would also work); \
             generator excludes this case"
        );
    }

    #[test]
    #[should_panic(expected = "not in production cycle")]
    fn panics_on_unknown_current() {
        cycle_click_count("UNKNOWN", "TODO");
    }

    #[test]
    #[should_panic(expected = "not in production cycle")]
    fn panics_on_unknown_target() {
        cycle_click_count("TODO", "UNKNOWN");
    }
}

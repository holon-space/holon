//! Transition: toggle the task state of a block via the StateToggle widget
//! path.
//!
//! @pbt rung input-pipeline
//!   `apply_toggle_state_to_sut`: wait_for_widget_kind(state_toggle) +
//!   click_entity through the production driver.
//! @pbt covers task-state-cycle — state_toggle click -> task-state cycle
//!
//! Mirrors the legacy logic split across `state_machine.rs:941-1054`
//! (generator), `state_machine.rs:3236-3262` (precondition),
//! `state_machine.rs:2519-2533` (ref-state apply),
//! `sut.rs:2176-2359` (SUT apply), and
//! `transition_budgets.rs:279-281` (expected SQL).

use std::time::Duration;

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusRoots;
use holon_pbt_core::capabilities::RefLayout;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::RefTaskStateToggle;
use holon_pbt_core::capabilities::SutDriver;
use holon_pbt_core::capabilities::SutLayout;
use holon_pbt_core::capabilities::SutMutate;
use holon_pbt_core::types::CycleTarget;
use holon_pbt_core::types::TASK_STATE_CYCLE;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

/// Compute how many state_toggle clicks advance the cycle from
/// `current` to `target`. Total over any `current` keyword: custom
/// keywords from a doc `#+TODO:` set (axis 5 — STARTED/NEXT/WAITING/…)
/// are off-cycle, and production's `cycle_state` falls back to index 0
/// (`unwrap_or(0)` in render_eval.rs) for them — the first click lands
/// on TODO regardless of the keyword, and reaching the empty state
/// takes the full cycle.
///
/// Returns `0` only for known-current no-op transitions (`current ==
/// target`), which the generator excludes; the SutHandle adapter
/// asserts `>0` defensively as a generator-drift guard.
pub fn cycle_click_count(current: &str, target: CycleTarget) -> u8 {
    let len = TASK_STATE_CYCLE.len();
    let tgt_idx = target.idx();
    match TASK_STATE_CYCLE.iter().position(|s| *s == current) {
        Some(cur_idx) => ((tgt_idx + len - cur_idx) % len) as u8,
        None if tgt_idx == 0 => len as u8,
        None => tgt_idx as u8,
    }
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
    sut: &S,
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

use holon_api::EntityUri;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

/// Toggle the task state of a block via the StateToggle widget.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ToggleState {
    pub block_id: EntityUri,
    pub new_state: CycleTarget,
}

impl<
    R: RefLifecycle
        + RefFocus
        + RefFocusRoots
        + RefBlockTree
        + RefLayout
        + RefTaskState
        + RefTaskStateToggle,
> TransitionFactory<R> for ToggleState
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // The candidate set — blocks rendering an interactive `state_toggle`
        // widget in Main — is computed by interpreting the render expr, which
        // lives in `RefTaskStateToggle::rendered_state_toggle_ids`.
        let pairs: Vec<(EntityUri, CycleTarget)> = state
            .rendered_state_toggle_ids()
            .into_iter()
            .filter(|id| {
                ToggleState {
                    block_id: id.clone(),
                    new_state: CycleTarget::Clear, // dummy for preconditions check
                }
                .preconditions(state)
                .is_good()
            })
            .flat_map(|id| {
                let current_state = state.task_state_of(&id).unwrap_or_default();
                let bid = id.clone();
                // A custom doc keyword (off-cycle, axis 5) never equals a
                // cycle member, so all four targets remain candidates.
                CycleTarget::ALL
                    .into_iter()
                    .filter(move |t| t.keyword() != current_state)
                    .map(move |t| (bid.clone(), t))
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

impl<R: RefLifecycle + RefFocus + RefFocusRoots + RefBlockTree + RefLayout + RefTaskStateToggle>
    TransitionRef<R> for ToggleState
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let focus_roots = state.expected_focus_root_ids(CapRegion::Main);
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            // `state_toggle` only exists when the block renders interactively
            // (default layout); a custom `index.org` layout can omit it. See
            // RefLifecycle::renders_block_interactively.
            check(
                state.renders_block_interactively(&self.block_id),
                Reason::BlocksNotInteractiveUnderLayout,
            ),
            check(
                state.current_focus(CapRegion::Main).is_some(),
                Reason::NoFocusInMain,
            ),
            // The toggled row must be VISIBLE in Main — i.e. the block is a
            // focus root OR a descendant of one (production renders every row
            // under the focused page, and a user can click the `state_toggle`
            // on any such interactive row). Requiring the block to BE a focus
            // root was stricter than prod (there is no block-zoom gesture that
            // makes a child block a focus root), which made ToggleState vacuous
            // (only pages can be focus roots, and pages aren't task rows). The
            // `is_descendant_of_any` self-or-descendant walk is the same faithful
            // visibility predicate `main_editable_descendants` uses.
            check(
                state.is_descendant_of_any(&self.block_id, &focus_roots),
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
                !state.is_layout_block(&self.block_id),
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

    fn apply_to_ref(&self, state: &mut R) {
        // Undo snapshot + `Update { task_state }` mutation live in the ref cap.
        state.apply_toggle_state(&self.block_id, self.new_state);
    }
}

crate::cap_transition! {
    ToggleState: SutMutate,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.toggle_state(&me.block_id, me.new_state).await;
    }
    sql_budget: |_me, state| {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        expected_sql_for_kind(MutationKind::Update, watches, blocks, docs)
    }
}

#[cfg(test)]
mod cycle_click_count_tests {
    use super::CycleTarget;
    use super::CycleTarget::Clear;
    use super::CycleTarget::Doing;
    use super::CycleTarget::Done;
    use super::CycleTarget::Todo;
    use super::TASK_STATE_CYCLE;
    use super::cycle_click_count;

    #[test]
    fn cycle_order_matches_production() {
        // Locked to sql_operation_provider.rs:1525-1526. If production
        // changes, this test breaks loudly and we update both sides.
        assert_eq!(TASK_STATE_CYCLE, &["", "TODO", "DOING", "DONE"]);
        let target_keywords: Vec<&str> = CycleTarget::ALL.iter().map(|t| t.keyword()).collect();
        assert_eq!(target_keywords, TASK_STATE_CYCLE);
    }

    #[test]
    fn single_step_clicks() {
        assert_eq!(cycle_click_count("", Todo), 1);
        assert_eq!(cycle_click_count("TODO", Doing), 1);
        assert_eq!(cycle_click_count("DOING", Done), 1);
        assert_eq!(cycle_click_count("DONE", Clear), 1);
    }

    #[test]
    fn multi_step_clicks() {
        assert_eq!(cycle_click_count("", Doing), 2);
        assert_eq!(cycle_click_count("", Done), 3);
        assert_eq!(cycle_click_count("TODO", Done), 2);
    }

    #[test]
    fn wraps_around_cycle() {
        // DONE → "" → TODO is 2 clicks (wraps through empty).
        assert_eq!(cycle_click_count("DONE", Todo), 2);
        assert_eq!(cycle_click_count("DONE", Doing), 3);
        assert_eq!(cycle_click_count("DOING", Todo), 3);
    }

    #[test]
    fn same_state_returns_full_cycle_length() {
        // SutHandle adapter asserts > 0 to catch the no-op transition
        // case; the generator already excludes these. This test pins
        // the modular-arithmetic edge case so a refactor that breaks
        // it fails loudly.
        assert_eq!(
            cycle_click_count("TODO", Todo),
            0,
            "same-state click_count is 0 (full cycle would also work); generator excludes this \
             case"
        );
    }

    #[test]
    fn custom_keyword_current_mirrors_production_index_0_fallback() {
        // Production's cycle_state treats an unknown keyword as index 0,
        // so the first click lands on TODO; Clear takes a full cycle.
        assert_eq!(cycle_click_count("STARTED", Todo), 1);
        assert_eq!(cycle_click_count("STARTED", Doing), 2);
        assert_eq!(cycle_click_count("WAITING", Done), 3);
        assert_eq!(cycle_click_count("NEXT", Clear), 4);
    }

    #[test]
    fn capture_compat_serde_round_trip() {
        // Old captures store new_state as the plain keyword string.
        for t in CycleTarget::ALL {
            let json = serde_json::to_string(&t).unwrap();
            assert_eq!(json, format!("{:?}", t.keyword()));
            assert_eq!(serde_json::from_str::<CycleTarget>(&json).unwrap(), t);
        }
        assert!(
            serde_json::from_str::<CycleTarget>("\"STARTED\"")
                .unwrap_err()
                .to_string()
                .contains("not a production-cycle keyword")
        );
    }
}

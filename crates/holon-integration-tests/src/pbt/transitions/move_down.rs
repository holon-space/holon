//! Transition: move the focused block down (swap with its next sibling).
//!
//! @pbt rung input-pipeline
//!   KEYSTONE: send_block_chord resolves the bound Alt+Down chord from the
//!   live registry -> bubble_input -> ExecuteOperation; fixed-id slices fall
//!   back to OpDispatchWriter (dispatch floor).
//! @pbt covers reorder-chord-down — Alt+Down chord -> move_down reducer
//!
//! Mirrors the legacy logic split across `state_machine.rs:1154-1169`
//! (generator), `state_machine.rs:3359-3373` (precondition),
//! `state_machine.rs:2673-2677` (ref-state apply),
//! `sut.rs:3562-3567` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefGlobalFocus;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

/// Move the focused block down: swap its sort_key with its next sibling's.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MoveDown {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn move_down_preconditions<R: RefBlockTree + RefFocus + RefLifecycle>(
    block_id: &EntityUri,
    state: &R,
) -> Validated<(), Reason> {
    let focus_roots = state.focus_root_ids(CapRegion::Main);
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
        // Block-interaction transitions need the block to render as an
        // interactive widget (ops/draggable) reactively over the navigated
        // focus. Only the default layout does; custom `index.org` query
        // layouts don't (see RefLifecycle::renders_block_interactively).
        check(
            state.renders_block_interactively(block_id),
            Reason::BlocksNotInteractiveUnderLayout,
        ),
    ];

    let focus = state.current_focus(CapRegion::Main);
    checks.push(check(
        focus.as_ref() == Some(block_id),
        Reason::FocusedIsNotSelf,
    ));

    if focus.as_ref() == Some(block_id) {
        checks.push(check(
            state.block_content(block_id).is_some(),
            Reason::FocusedBlockMissing,
        ));
        checks.push(check(
            state.is_text_block(block_id) && !state.is_page_block(block_id),
            Reason::FocusedNotText,
        ));
        checks.push(check(
            state.is_focusable(block_id),
            Reason::FocusedNotFocusable,
        ));
        checks.push(check(
            !state.is_layout_block(block_id),
            Reason::FocusedInLayoutBlocks,
        ));
        checks.push(check(
            state.is_descendant_of_any(block_id, &focus_roots),
            Reason::FocusedNotDescendantOfFocusRoot,
        ));
        checks.push(check(
            state.next_sibling(block_id).is_some(),
            Reason::NoNextSibling,
        ));
    }
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn move_down_weighted_generator<R: RefBlockTree + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<MoveDown>), Reason> {
    let Some(focus_str) = state.current_focus(CapRegion::Main) else {
        return Validated::fail(Reason::NoFocusInMain);
    };
    move_down_preconditions(&focus_str, state).map(|()| {
        let instance = MoveDown {
            block_id: focus_str,
        };
        // F16: raise structural chord weight 1 → 20 (was ~1/180 vs split=100).
        (20, Just(instance).boxed())
    })
}

pub fn move_down_apply_to_ref<
    R: RefBlockTree + RefBlockTreeMut + RefFocus + RefGlobalFocus + RefFocusMut + RefEditorMirrorMut,
>(
    block_id: &EntityUri,
    state: &mut R,
) {
    // Model the chord-dispatch click (see mod.rs::model_chord_click_focus).
    super::model_chord_click_focus(block_id, state);
    state.push_undo_snapshot();
    let next_id = state
        .next_sibling(block_id)
        .expect("MoveDown: next sibling required (precondition)");
    state.swap_siblings(block_id, &next_id);
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl<R: RefBlockTree + RefFocus + RefLifecycle> TransitionFactory<R> for MoveDown {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        move_down_weighted_generator(state)
    }
}

impl<
    R: RefBlockTree
        + RefBlockTreeMut
        + RefFocus
        + RefGlobalFocus
        + RefFocusMut
        + RefEditorMirrorMut
        + RefLifecycle,
> TransitionRef<R> for MoveDown
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        move_down_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        move_down_apply_to_ref(&self.block_id, state);
    }
}

crate::cap_transition! {
    MoveDown: SutBlockTreeWrite,
    where R: [ RefBlockTree + RefFocus + RefLifecycle ],
    |me, _state, sut| {
        sut.apply_move_down(&me.block_id).await;
    }
    sql_budget: |_me, state| {
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watch_count(),
            state.block_count(),
            state.document_count(),
        );
        sql.tolerance += 5; // extra margin for ordering operations
        sql
    }
}

//! Transition: move the focused block down (swap with its next sibling).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1154-1169` (generator),
//! `state_machine.rs:3359-3373` (precondition),
//! `state_machine.rs:2673-2677` (ref-state apply),
//! `sut.rs:3562-3567` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use holon_pbt_core::capabilities::{
    CapBlockId, CapRegion, RefBlockTree, RefBlockTreeMut, RefFocus, RefLifecycle,
};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

use holon_api::EntityUri;

/// Move the focused block down: swap its sort_key with its next sibling's.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MoveDown {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn move_down_preconditions<R: RefBlockTree + RefFocus + RefLifecycle>(
    block_id: &CapBlockId,
    state: &R,
) -> Validated<(), Reason> {
    let focus_roots = state.focus_root_ids(CapRegion::Main);
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
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
        let block_id =
            EntityUri::parse(&focus_str).expect("focused id must parse as EntityUri in wide PBT");
        let instance = MoveDown { block_id };
        (1, Just(instance).boxed())
    })
}

pub fn move_down_apply_to_ref<R: RefBlockTree + RefBlockTreeMut>(
    block_id: &CapBlockId,
    state: &mut R,
) {
    state.push_undo_snapshot();
    let next_id = state
        .next_sibling(block_id)
        .expect("MoveDown: next sibling required (precondition)");
    state.swap_siblings(block_id, &next_id);
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl E2ETransitionFactory for MoveDown {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        move_down_weighted_generator(state)
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for MoveDown {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        move_down_preconditions(&self.block_id.as_str().to_string(), state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        move_down_apply_to_ref(&self.block_id.as_str().to_string(), state);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_move_down(&self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.active_watches.len(),
            state.block_state.blocks.len(),
            state.documents.len(),
        );
        sql.tolerance += 5; // extra margin for ordering operations
        sql
    }
}

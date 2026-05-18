//! Transition: join a block into its previous sibling (or parent).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1194-1245` (generator),
//! `state_machine.rs:3438-3486` (precondition),
//! `state_machine.rs:2693-2718` (ref-state apply),
//! `sut.rs:4129-4147` (SUT apply), and
//! `transition_budgets.rs:314-323` (expected SQL).

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::{
    CapBlockId, CapCursor, CapRegion, RefBlockTree, RefBlockTreeMut, RefFocus, RefFocusMut,
    RefLifecycle,
};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::{Reason, check};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, MutationKind, REACTIVE_BASE, expected_sql_for_kind,
};

/// Join a block into its previous text sibling, or (when first child) into
/// its non-layout text parent. Mirrors Backspace-at-position-0 semantics.
#[derive(Clone, Debug)]
pub struct JoinBlock {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn join_block_preconditions<R: RefBlockTree + RefFocus + RefLifecycle>(
    block_id: &CapBlockId,
    state: &R,
) -> Validated<(), Reason> {
    let focus_roots = state.focus_root_ids(CapRegion::Main);
    let focused = state.current_focus(CapRegion::Main);
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
    ];
    checks.push(check(
        focused.as_ref() == Some(block_id),
        Reason::FocusedIsNotSelf,
    ));
    checks.push(check(
        state.block_content(block_id).is_some(),
        Reason::FocusedBlockMissing,
    ));
    checks.push(check(state.is_text_block(block_id), Reason::FocusedNotText));
    checks.push(check(
        !state.is_layout_block(block_id),
        Reason::FocusedInLayoutBlocks,
    ));
    checks.push(check(
        state.is_descendant_of_any(block_id, &focus_roots),
        Reason::FocusedNotDescendantOfFocusRoot,
    ));

    // Case 1: previous text sibling exists → join into prev sibling.
    let prev_text = state
        .previous_sibling(block_id)
        .map(|prev| state.is_text_block(&prev))
        .unwrap_or(false);

    // Case 2: no previous sibling AND parent is a non-layout text block.
    let parent_ok = if !prev_text && state.previous_sibling(block_id).is_none() {
        match state.parent_of(block_id) {
            Some(parent) => state.is_text_block(&parent) && !state.is_layout_block(&parent),
            None => false,
        }
    } else {
        prev_text
    };

    checks.push(check(prev_text || parent_ok, Reason::PreconditionFailed));
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn join_block_weighted_generator<R: RefBlockTree + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<JoinBlock>), Reason> {
    let Some(focus_str) = state.current_focus(CapRegion::Main) else {
        return Validated::fail(Reason::NoFocusInMain);
    };
    join_block_preconditions(&focus_str, state).map(|()| {
        let block_id =
            EntityUri::parse(&focus_str).expect("focused id must parse as EntityUri in wide PBT");
        let instance = JoinBlock { block_id };
        (1, Just(instance).boxed())
    })
}

pub fn join_block_apply_to_ref<R: RefBlockTree + RefBlockTreeMut + RefFocusMut>(
    block_id: &CapBlockId,
    state: &mut R,
) {
    state.push_undo_snapshot();
    // Determine the merge target before mutation: prev sibling if
    // present, otherwise the parent block (child→parent join).
    let target_id = state.previous_sibling(block_id).unwrap_or_else(|| {
        state
            .parent_of(block_id)
            .expect("JoinBlock: parent required")
    });
    state.join_block(block_id);
    // Focus moves to the merge target; cursor lands at the join boundary,
    // but the reference model tracks (line, column) — reset to start to
    // match SplitBlock's behaviour.
    state.set_focus(CapRegion::Main, target_id, CapCursor::default());
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl E2ETransitionFactory for JoinBlock {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        join_block_weighted_generator(state)
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for JoinBlock {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        join_block_preconditions(&self.block_id.as_str().to_string(), state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        join_block_apply_to_ref(&self.block_id.as_str().to_string(), state);
    }

    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_join_block(&self.block_id, ref_state).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        let update = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
        let delete = expected_sql_for_kind(MutationKind::Delete, watches, blocks, docs);
        ExpectedSql {
            reads: update.reads + delete.reads - REACTIVE_BASE,
            writes: update.writes + delete.writes,
            ddl: 0,
            tolerance: update.tolerance + delete.tolerance,
        }
    }
}

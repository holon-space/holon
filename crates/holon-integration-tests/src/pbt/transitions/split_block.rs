//! Transition: split a block at a byte position.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1171-1191` (generator),
//! `state_machine.rs:3426-3436` (precondition),
//! `state_machine.rs:2687-2691` (ref-state apply),
//! `sut.rs:4052-4127` (SUT apply), and
//! `transition_budgets.rs:303-312` (expected SQL).

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::{
    CapBlockId, CapCursor, CapRegion, RefBlockTree, RefBlockTreeMut, RefFocusMut, RefLifecycle,
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

/// Split an editable text block at a byte position.
/// Only the currently focused (editable) block is a candidate.
#[derive(Clone, Debug)]
pub struct SplitBlock {
    pub block_id: EntityUri,
    pub position: usize,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn split_block_preconditions<R: RefBlockTree + RefLifecycle>(
    block_id: &CapBlockId,
    position: usize,
    state: &R,
) -> Validated<(), Reason> {
    let focus_roots = state.focus_root_ids(CapRegion::Main);
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
    ];
    let content = state.block_content(block_id);
    checks.push(check(content.is_some(), Reason::FocusedBlockMissing));
    checks.push(check(state.is_text_block(block_id), Reason::FocusedNotText));
    if let Some(text) = content {
        checks.push(check(position <= text.len(), Reason::PreconditionFailed));
    }
    checks.push(check(
        !state.is_layout_block(block_id),
        Reason::FocusedInLayoutBlocks,
    ));
    checks.push(check(
        state.is_descendant_of_any(block_id, &focus_roots),
        Reason::FocusedNotDescendantOfFocusRoot,
    ));
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn split_block_weighted_generator<R: RefBlockTree + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<SplitBlock>), Reason> {
    let mut candidates: Vec<(EntityUri, usize)> = vec![];
    for id in state.main_editable_descendants() {
        if let Some(text) = state.block_content(&id) {
            let content_len = text.len();
            for position in 0..=content_len {
                if split_block_preconditions(&id, position, state).is_good() {
                    if let Ok(uri) = EntityUri::parse(&id) {
                        candidates.push((uri, position));
                    }
                }
            }
        }
    }
    check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
        let strat = prop::sample::select(candidates)
            .prop_map(|(block_id, position)| SplitBlock { block_id, position })
            .boxed();
        // High weight: editing transitions are starved unless Main is
        // populated; SplitBlock exercises the Enter → split path.
        (100, strat)
    })
}

pub fn split_block_apply_to_ref<R: RefBlockTreeMut + RefFocusMut>(
    block_id: &CapBlockId,
    position: usize,
    state: &mut R,
) {
    state.push_undo_snapshot();
    let new_block_id = state.split_block(block_id, position);
    // Production issues an editor_focus follow-up that moves keyboard
    // focus to the new block at position 0
    // (`traits.rs::split_block` → editor_focus_op). Mirror that so
    // subsequent transitions and post-step invariants see the right
    // focused block.
    state.set_focus(CapRegion::Main, new_block_id, CapCursor::default());
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl E2ETransitionFactory for SplitBlock {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        split_block_weighted_generator(state)
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for SplitBlock {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        split_block_preconditions(&self.block_id.as_str().to_string(), self.position, state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        split_block_apply_to_ref(&self.block_id.as_str().to_string(), self.position, state);
    }

    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_split_block(&self.block_id, self.position, ref_state)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        let update = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
        let create = expected_sql_for_kind(MutationKind::Create, watches, blocks, docs);
        ExpectedSql {
            reads: update.reads + create.reads - REACTIVE_BASE,
            writes: update.writes + create.writes,
            ddl: 0,
            tolerance: update.tolerance + create.tolerance,
        }
    }
}

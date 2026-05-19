//! Transition: split a block at a byte position.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1171-1191` (generator),
//! `state_machine.rs:3426-3436` (precondition),
//! `state_machine.rs:2687-2691` (ref-state apply),
//! `sut.rs:4052-4127` (SUT apply), and
//! `transition_budgets.rs:303-312` (expected SQL).

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::capabilities::{
    CapCursor, CapRegion, RefBlockTree, RefBlockTreeMut, RefFocusMut, RefLifecycle,
    SutBlockTreeWrite, SutDriver, SutLayout,
};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use std::time::Duration;
use validated::Validated;

// ── Capability-bound free function (Phase C input pipeline) ──────────

/// SUT-side input-pipeline body of `SplitBlock`. Bound on
/// `SutLayout + SutDriver`. Drives the physical sequence a real user
/// would perform: ensure target is rendered as an editable widget,
/// click to focus, then type `home` + N×`right` + `Enter`.
///
/// The Enter handler at `editor_view.rs:543-575` is a capture_action
/// that reads `input.read(cx).cursor()` from the live `InputState` and
/// dispatches `split_block` against the focused editor — a separate
/// code path from the bubble-phase chord resolver that `Ctrl+x` hits.
/// Driving Enter exercises that production path.
///
/// `wait_for_widget_kind` is a stronger precondition than
/// `wait_for_bounds`: confirms the target is rendered as the
/// interactive `editable_text` or its read-only `rendered_text` sibling
/// (a click can either focus the editor or promote the read-only
/// variant). Mismatches surface here instead of as a confusing focus
/// timeout 1 s later.
///
/// `wait_for_engine_focus` after the click prevents focus drift: if the
/// click silently focuses a different block, Enter would fire against
/// the wrong editor and `split_block` would split the wrong content.
///
/// Caller responsibility: pre-condition gates that depend on
/// pre-transition state (`wait_for_children_settled` on the original
/// parent) and post-transition assertions (block-count sync,
/// synthetic-id mapping). Those stay in the SutHandle adapter because
/// they read from E2ESut-internal state.
pub async fn apply_split_block_input_pipeline_to_sut<S: SutLayout + SutDriver>(
    sut: &mut S,
    id: &EntityUri,
    position: usize,
) {
    sut.wait_for_widget_kind(
        id,
        &["editable_text", "rendered_text"],
        Duration::from_secs(2),
    )
    .await
    .unwrap_or_else(|e| {
        panic!("[SplitBlock] target {id} not rendered as editable_text/rendered_text: {e}")
    });
    sut.click_entity(id, "main")
        .await
        .unwrap_or_else(|e| panic!("[SplitBlock] click_entity failed for {id}: {e}"));
    sut.wait_for_engine_focus(id, Duration::from_secs(1))
        .await
        .unwrap_or_else(|e| {
            panic!(
                "[SplitBlock] click_entity did not focus {id} before Enter \
                 — split would have hit the wrong block: {e}"
            )
        });
    sut.send_raw_keystroke("home", &[])
        .await
        .unwrap_or_else(|e| panic!("[SplitBlock] home failed: {e}"));
    for _ in 0..position {
        sut.send_raw_keystroke("right", &[])
            .await
            .unwrap_or_else(|e| panic!("[SplitBlock] right failed: {e}"));
    }
    sut.send_raw_keystroke("enter", &[])
        .await
        .unwrap_or_else(|e| panic!("[SplitBlock] enter failed: {e}"));
}

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, MutationKind, REACTIVE_BASE, expected_sql_for_kind,
};

/// Split an editable text block at a byte position.
/// Only the currently focused (editable) block is a candidate.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SplitBlock {
    pub block_id: EntityUri,
    pub position: usize,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn split_block_preconditions<R: RefBlockTree + RefLifecycle>(
    block_id: &EntityUri,
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
                    candidates.push((id.clone(), position));
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
    block_id: &EntityUri,
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

impl TransitionFactory<ReferenceState> for SplitBlock {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        split_block_weighted_generator(state)
    }
}

impl TransitionRef<ReferenceState> for SplitBlock {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        split_block_preconditions(&self.block_id, self.position, state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        split_block_apply_to_ref(&self.block_id, self.position, state);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutBlockTreeWrite> TransitionImpl<ReferenceState, S> for SplitBlock {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_split_block(&self.block_id, self.position).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for SplitBlock {
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

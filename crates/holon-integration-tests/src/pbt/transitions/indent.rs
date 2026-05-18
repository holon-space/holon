//! Transition: indent the focused block (make it a child of its previous sibling).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1106-1121` (generator),
//! `state_machine.rs:3314-3328` (precondition),
//! `state_machine.rs:2648-2660` (ref-state apply),
//! `sut.rs:3541-3546` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use holon_api::EntityUri;
use holon_pbt_core::capabilities::{
    CapBlockId, CapRegion, RefBlockTree, RefBlockTreeMut, RefLifecycle,
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

/// Indent the focused block: re-parent it under its previous sibling via the
/// `Alt+Right` / Tab chord.
#[derive(Clone, Debug)]
pub struct Indent {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn indent_preconditions<R: RefBlockTree + RefLifecycle>(
    block_id: &CapBlockId,
    state: &R,
) -> Validated<(), Reason> {
    let focus_roots = state.focus_root_ids(CapRegion::Main);
    let mut checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
    ];
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
    checks.push(check(
        state.previous_sibling(block_id).is_some(),
        Reason::NoPreviousSibling,
    ));
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn indent_weighted_generator<R: RefBlockTree + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<Indent>), Reason> {
    let candidates: Vec<EntityUri> = state
        .main_editable_descendants()
        .into_iter()
        .filter(|id| indent_preconditions(id, state).is_good())
        .filter_map(|id| EntityUri::parse(&id).ok())
        .collect();
    check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
        let strat = prop::sample::select(candidates)
            .prop_map(|block_id| Indent { block_id })
            .boxed();
        (1, strat)
    })
}

pub fn indent_apply_to_ref<R: RefBlockTree + RefBlockTreeMut>(
    block_id: &CapBlockId,
    state: &mut R,
) {
    state.push_undo_snapshot();
    let prev_id = state
        .previous_sibling(block_id)
        .expect("Indent: previous sibling required (precondition)");
    // Production indent re-parents the block under its previous
    // sibling, anchored after that parent's current last child —
    // i.e. it lands at the end of the new sibling group.
    let after = state.sorted_children(&prev_id).last().cloned();
    state.move_block(block_id, prev_id, after.as_ref());
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl E2ETransitionFactory for Indent {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        indent_weighted_generator(state)
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for Indent {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        indent_preconditions(&self.block_id.as_str().to_string(), state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        indent_apply_to_ref(&self.block_id.as_str().to_string(), state);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_indent(&self.block_id).await;
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

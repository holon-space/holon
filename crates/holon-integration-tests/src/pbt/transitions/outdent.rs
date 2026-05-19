//! Transition: outdent the focused block (move it up to its grandparent level).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1122-1137` (generator),
//! `state_machine.rs:3329-3343` (precondition),
//! `state_machine.rs:2662-2665` (ref-state apply),
//! `sut.rs:3548-3553` (SUT apply), and
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

/// Outdent the focused block: move it up one level to its grandparent via the
/// `Alt+Left` / Shift+Tab chord.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Outdent {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn outdent_preconditions<R: RefBlockTree + RefLifecycle>(
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
        state.grandparent(block_id).is_some(),
        Reason::NoGrandparent,
    ));
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn outdent_weighted_generator<R: RefBlockTree + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<Outdent>), Reason> {
    let candidates: Vec<EntityUri> = state
        .main_editable_descendants()
        .into_iter()
        .filter(|id| outdent_preconditions(id, state).is_good())
        .filter_map(|id| EntityUri::parse(&id).ok())
        .collect();
    check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
        let strat = prop::sample::select(candidates)
            .prop_map(|block_id| Outdent { block_id })
            .boxed();
        (1, strat)
    })
}

pub fn outdent_apply_to_ref<R: RefBlockTreeMut>(block_id: &CapBlockId, state: &mut R) {
    state.push_undo_snapshot();
    state.outdent(block_id);
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl E2ETransitionFactory for Outdent {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        outdent_weighted_generator(state)
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for Outdent {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        outdent_preconditions(&self.block_id.as_str().to_string(), state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        outdent_apply_to_ref(&self.block_id.as_str().to_string(), state);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_outdent(&self.block_id).await;
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

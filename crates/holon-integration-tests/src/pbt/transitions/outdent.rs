//! Transition: outdent the focused block (move it up to its grandparent level).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1122-1137`
//! (generator), `state_machine.rs:3329-3343` (precondition),
//! `state_machine.rs:2662-2665` (ref-state apply),
//! `sut.rs:3548-3553` (SUT apply), and
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

/// Outdent the focused block: move it up one level to its grandparent via the
/// `Alt+Left` / Shift+Tab chord.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Outdent {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn outdent_preconditions<R: RefBlockTree + RefLifecycle>(
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
        .collect();
    check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
        let strat = prop::sample::select(candidates)
            .prop_map(|block_id| Outdent { block_id })
            .boxed();
        (1, strat)
    })
}

pub fn outdent_apply_to_ref<
    R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefEditorMirrorMut,
>(
    block_id: &EntityUri,
    state: &mut R,
) {
    // Model the chord-dispatch click (see mod.rs::model_chord_click_focus).
    super::model_chord_click_focus(block_id, state);
    state.push_undo_snapshot();
    state.outdent(block_id);
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl<R: RefBlockTree + RefLifecycle> TransitionFactory<R> for Outdent {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        outdent_weighted_generator(state)
    }
}

impl<R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefEditorMirrorMut + RefLifecycle>
    TransitionRef<R> for Outdent
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        outdent_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        outdent_apply_to_ref(&self.block_id, state);
    }
}

crate::cap_transition! {
    Outdent: SutBlockTreeWrite,
    where R: [ RefBlockTree + RefLifecycle ],
    |me, _state, sut| {
        sut.apply_outdent(&me.block_id).await;
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

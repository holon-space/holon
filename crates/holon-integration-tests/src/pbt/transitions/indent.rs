//! Transition: indent the focused block (make it a child of its previous
//! sibling).
//!
//! @pbt rung input-pipeline
//!   KEYSTONE: KeystrokeBlockTreeWriter drives the bound chord (Tab) through
//!   the production chord-resolution path. FIXED-ID lib slices (no resolver)
//!   fall back to OpDispatchWriter raw op dispatch (dispatch floor).
//! @pbt covers indent-chord — indent chord -> bubble_input -> structural reducer
//!
//! Mirrors the legacy logic split across `state_machine.rs:1106-1121`
//! (generator), `state_machine.rs:3314-3328` (precondition),
//! `state_machine.rs:2648-2660` (ref-state apply),
//! `sut.rs:3541-3546` (SUT apply), and
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

/// Indent the focused block: re-parent it under its previous sibling via the
/// `Alt+Right` / Tab chord.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Indent {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn indent_preconditions<R: RefBlockTree + RefLifecycle>(
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
        .collect();
    check(!candidates.is_empty(), Reason::PreconditionFailed).map(|_| {
        let strat = prop::sample::select(candidates)
            .prop_map(|block_id| Indent { block_id })
            .boxed();
        (1, strat)
    })
}

pub fn indent_apply_to_ref<
    R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefEditorMirrorMut,
>(
    block_id: &EntityUri,
    state: &mut R,
) {
    // The SUT dispatches this op via chord, which CLICKS the block first —
    // focusing it and opening its editor. Model that click or
    // `inv-focus-matches-ref` diverges whenever no editor was open before.
    super::model_chord_click_focus(block_id, state);
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

impl<R: RefBlockTree + RefLifecycle> TransitionFactory<R> for Indent {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        indent_weighted_generator(state)
    }
}

impl<R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefEditorMirrorMut + RefLifecycle>
    TransitionRef<R> for Indent
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        indent_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        indent_apply_to_ref(&self.block_id, state);
    }
}

crate::cap_transition! {
    Indent: SutBlockTreeWrite,
    where R: [ RefBlockTree + RefLifecycle ],
    |me, _state, sut| {
        sut.apply_indent(&me.block_id).await;
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

//! Transition: move the focused block up (swap with its previous sibling).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1138-1153`
//! (generator), `state_machine.rs:3344-3358` (precondition),
//! `state_machine.rs:2667-2671` (ref-state apply),
//! `sut.rs:3555-3560` (SUT apply), and
//! `transition_budgets.rs:298-302` (expected SQL).

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefFocusMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutBlockTreeWrite;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;
use crate::pbt::validation::Reason;
use crate::pbt::validation::check;

/// Move the focused block up: swap its sort_key with its previous sibling's.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MoveUp {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn move_up_preconditions<R: RefBlockTree + RefFocus + RefLifecycle>(
    block_id: &EntityUri,
    state: &R,
) -> Validated<(), Reason> {
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
        let focus_roots = state.focus_root_ids(CapRegion::Main);
        checks.push(check(state.is_text_block(block_id), Reason::FocusedNotText));
        checks.push(check(!state.is_page_block(block_id), Reason::FocusedIsPage));
        checks.push(check(
            state.is_focusable(block_id),
            Reason::FocusedNotFocusable,
        ));
        checks.push(check(
            !state.is_no_content_update(block_id),
            Reason::FocusedInNoContentUpdate,
        ));
        checks.push(check(
            state.is_descendant_of_any(block_id, &focus_roots),
            Reason::FocusedNotDescendantOfFocusRoot,
        ));
        checks.push(check(
            state.previous_sibling(block_id).is_some(),
            Reason::NoPreviousSibling,
        ));
    }
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn move_up_weighted_generator<R: RefBlockTree + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<MoveUp>), Reason> {
    let Some(focus_str) = state.current_focus(CapRegion::Main) else {
        return Validated::fail(Reason::NoFocusInMain);
    };
    move_up_preconditions(&focus_str, state).map(|()| {
        let instance = MoveUp {
            block_id: focus_str,
        };
        (1, Just(instance).boxed())
    })
}

pub fn move_up_apply_to_ref<
    R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefEditorMirrorMut,
>(
    block_id: &EntityUri,
    state: &mut R,
) {
    // Model the chord-dispatch click (see mod.rs::model_chord_click_focus).
    super::model_chord_click_focus(block_id, state);
    state.push_undo_snapshot();
    let prev_id = state
        .previous_sibling(block_id)
        .expect("MoveUp: previous sibling required (precondition)");
    state.swap_siblings(block_id, &prev_id);
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl<R: RefBlockTree + RefFocus + RefLifecycle> TransitionFactory<R> for MoveUp {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutBlockTreeWrite,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        move_up_weighted_generator(state)
    }
}

impl<R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefEditorMirrorMut + RefLifecycle>
    TransitionRef<R> for MoveUp
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        move_up_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        move_up_apply_to_ref(&self.block_id, state);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutBlockTreeWrite> TransitionImpl<ReferenceState, S> for MoveUp {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_move_up(&self.block_id).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for MoveUp {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let mut sql = expected_sql_for_kind(
            MutationKind::Update,
            state.mcp.active_watches.len(),
            state.domain.block_state.blocks.len(),
            state.files.documents.len(),
        );
        sql.tolerance += 5; // extra margin for ordering operations
        sql
    }
}

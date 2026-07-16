//! Transition: join a block into its previous sibling (or parent).
//!
//! @pbt rung input-pipeline
//!   KEYSTONE: KeystrokeBlockTreeWriter drives Backspace-at-start via the
//!   editor keystroke path; fixed-id slices fall back to OpDispatchWriter
//!   (dispatch floor).
//! @pbt covers join-backspace — backspace-at-start -> join into previous
//! sibling
//!
//! Mirrors the legacy logic split across `state_machine.rs:1194-1245`
//! (generator), `state_machine.rs:3438-3486` (precondition),
//! `state_machine.rs:2693-2718` (ref-state apply),
//! `sut.rs:4129-4147` (SUT apply), and
//! `transition_budgets.rs:314-323` (expected SQL).

use holon_api::entity_uri::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapCursor;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
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
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

/// Join a block into its previous text sibling, or (when first child) into
/// its non-layout text parent. Mirrors Backspace-at-position-0 semantics.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct JoinBlock {
    pub block_id: EntityUri,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn join_block_preconditions<R: RefBlockTree + RefFocus + RefLifecycle>(
    block_id: &EntityUri,
    state: &R,
) -> Validated<(), Reason> {
    let focus_roots = state.focus_root_ids(CapRegion::Main);
    let focused = state.current_focus(CapRegion::Main);
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
        focused.as_ref() == Some(block_id),
        Reason::FocusedIsNotSelf,
    ));
    checks.push(check(
        state.block_content(block_id).is_some(),
        Reason::FocusedBlockMissing,
    ));
    checks.push(check(state.is_text_block(block_id), Reason::FocusedNotText));
    // A page (e.g. `block:journals`) is never backspace-joinable: it is the
    // root of its view, so the editor's `join_block` op finds no merge target
    // and the chord doesn't match — the SUT refuses to dispatch. The ref model
    // would otherwise allow it whenever the page has a previous *sibling* page
    // (`prev_text` true), diverging from the SUT. `move_up`/`move_down` exclude
    // pages the same way (`Reason::FocusedIsPage`).
    checks.push(check(!state.is_page_block(block_id), Reason::FocusedIsPage));
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
        let instance = JoinBlock {
            block_id: focus_str,
        };
        (1, Just(instance).boxed())
    })
}

pub fn join_block_apply_to_ref<R: RefBlockTree + RefBlockTreeMut + RefFocusMut>(
    block_id: &EntityUri,
    state: &mut R,
) {
    // Leaf-reversibility gate (matches U4's DeclaredIrreversible rule): the
    // engine only produces a compound inverse for a leaf join. Joining a block
    // that still has children is declared irreversible, so snapshotting here
    // would desync the ref undo stack from the engine's. Push only for leaves.
    if state.sorted_children(block_id).is_empty() {
        state.push_undo_snapshot();
    }
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

impl<R: RefBlockTree + RefFocus + RefLifecycle> TransitionFactory<R> for JoinBlock {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        join_block_weighted_generator(state)
    }
}

impl<R: RefBlockTree + RefBlockTreeMut + RefFocus + RefFocusMut + RefLifecycle> TransitionRef<R>
    for JoinBlock
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        join_block_preconditions(&self.block_id, state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        join_block_apply_to_ref(&self.block_id, state);
    }
}

crate::cap_transition! {
    JoinBlock: SutBlockTreeWrite,
    where R: [ RefBlockTree + RefFocus + RefLifecycle ],
    |me, _state, sut| {
        sut.apply_join_block(&me.block_id).await;
    }
    sql_budget: |_me, state| {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
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

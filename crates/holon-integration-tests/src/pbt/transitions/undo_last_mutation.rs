//! Transition: undo the last UI mutation.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1417-1418` (generator),
//! `state_machine.rs:3488` (precondition),
//! `state_machine.rs:2720-2726` (ref-state apply),
//! `sut.rs:4149-4157` (SUT apply), and
//! `transition_budgets.rs:339-343` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::{CursorPosition, ReferenceState};
use crate::pbt::transition_dispatch::SutHandle;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

/// Undo the last UI mutation via the engine's undo stack.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UndoLastMutation;

impl TransitionFactory<ReferenceState> for UndoLastMutation {
    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: undo routes through `ctx.engine().undo()` (the Turso
        // `BackendEngine`); the no-Turso wiring has no engine and no Loro undo
        // path is wired for a1. Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        UndoLastMutation
            .preconditions(state)
            .map(|_| (2, Just(UndoLastMutation).boxed()))
    }
}

impl TransitionRef<ReferenceState> for UndoLastMutation {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(!state.action.undo_stack.is_empty(), Reason::NoUndoHistory),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.pop_undo_to_redo();
        // Undo may restore different content — reset all cursors
        for region in state
            .ui
            .tab
            .focused_entity_id
            .keys()
            .cloned()
            .collect::<Vec<_>>()
        {
            state
                .ui
                .tab
                .focused_cursor
                .insert(region, CursorPosition::start());
        }
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for UndoLastMutation {
    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut S) {
        sut.apply_undo_last_mutation(ref_state).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for UndoLastMutation {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.mcp.active_watches.len();
        let blocks = state.domain.block_state.blocks.len();
        let docs = state.files.documents.len();
        let mut sql = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
        sql.tolerance += 5; // undo journal adds a few extra reads
        sql
    }
}

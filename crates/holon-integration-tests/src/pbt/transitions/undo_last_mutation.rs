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

use super::E2ETransitionImpl;
use crate::pbt::reference_state::{CursorPosition, ReferenceState};
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, MutationKind, expected_sql_for_kind};

/// Undo the last UI mutation via the engine's undo stack.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct UndoLastMutation;

impl E2ETransitionFactory for UndoLastMutation {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        UndoLastMutation
            .preconditions(state)
            .map(|_| (2, Just(UndoLastMutation).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for UndoLastMutation {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started, Reason::AppNotStarted),
            check(!state.undo_stack.is_empty(), Reason::NoUndoHistory),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.pop_undo_to_redo();
        // Undo may restore different content — reset all cursors
        for region in state.focused_entity_id.keys().cloned().collect::<Vec<_>>() {
            state.focused_cursor.insert(region, CursorPosition::start());
        }
    }

    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_undo_last_mutation(ref_state).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        let mut sql = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
        sql.tolerance += 5; // undo journal adds a few extra reads
        sql
    }
}

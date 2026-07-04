//! Transition: undo the last UI mutation.
//!
//! @pbt rung dispatch
//!   `undo_last_mutation` calls `engine.undo()` directly; cmd+z is unbound in
//!   production (undo-ruling), so no higher rung exists yet. OBSERVABILITY OK:
//!   the ref does a full block_state snapshot/restore, so block-tree
//!   correspondence observes wrong-content restores (cursor is only reset).
//! @pbt covers undo-stack — engine undo restores the pre-mutation block tree
//!
//! Mirrors the legacy logic split across `state_machine.rs:1417-1418`
//! (generator), `state_machine.rs:3488` (precondition),
//! `state_machine.rs:2720-2726` (ref-state apply),
//! `sut.rs:4149-4157` (SUT apply), and
//! `transition_budgets.rs:339-343` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutHistoryWrite;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::MutationKind;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::expected_sql_for_kind;

/// Undo the last UI mutation via the engine's undo stack.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I undo")]
pub struct UndoLastMutation;

impl<R: RefLifecycle + RefBlockTreeMut> TransitionFactory<R> for UndoLastMutation {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: undo routes through `ctx.engine().undo()` (the Turso
        // `BackendEngine`); the no-Turso wiring has no engine and no Loro undo
        // path is wired for a1. Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        UndoLastMutation
            .preconditions(state)
            // Weight 2 by default (unchanged); biased up under
            // `HOLON_PBT_UNDO_REDO_DENSITY=high` — see
            // `crate::pbt::undo_redo_density`.
            .map(|_| {
                (
                    crate::pbt::undo_redo_density::weight(2),
                    Just(UndoLastMutation).boxed(),
                )
            })
    }
}

impl<R: RefLifecycle + RefBlockTreeMut> TransitionRef<R> for UndoLastMutation {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.has_undo_history(), Reason::NoUndoHistory),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Pop undo→redo and reset every region cursor (undo may restore
        // different content) — the whole effect lives in the ref cap.
        state.undo_last_and_reset_cursors();
    }
}

crate::cap_transition! {
    UndoLastMutation: SutHistoryWrite,
    where R: [ RefLifecycle ],
    |_me, _state, sut| {
        sut.undo_last_mutation().await;
    }
    sql_budget: |_me, state| {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        let mut sql = expected_sql_for_kind(MutationKind::Update, watches, blocks, docs);
        sql.tolerance += 5; // undo journal adds a few extra reads
        sql
    }
}

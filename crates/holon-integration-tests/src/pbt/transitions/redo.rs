//! Transition: redo the last undone mutation.
//!
//! @pbt rung dispatch
//!   `redo` calls `engine.redo()` directly; no redo keybinding is bound in
//!   production (undo-ruling), so no higher rung exists yet to exercise.
//! @pbt covers redo-stack — engine redo restores the last undone mutation
//!
//! Mirrors the legacy logic split across `state_machine.rs:1420-1421`
//! (generator), `state_machine.rs:3489` (precondition),
//! `state_machine.rs:2727-2732` (ref-state apply),
//! `sut.rs:4159-4167` (SUT apply), and
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

/// Redo the last undone mutation via the engine's redo stack.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I redo")]
pub struct Redo;

impl<R: RefLifecycle + RefBlockTreeMut> TransitionFactory<R> for Redo {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // Turso-only: redo routes through `ctx.engine().redo()` (the Turso
        // `BackendEngine`); the no-Turso wiring has no engine and no Loro redo
        // path is wired for a1. Gate it out of {Loro} slices.
        ::holon_pbt_core::RequiredWiring::HasStorage(::holon_pbt_core::StorageAdapter::Turso)
    }
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Weight 2 by default (unchanged). `HOLON_PBT_UNDO_REDO_DENSITY=high`
        // biases it up so a measurement sweep can actually reach the undo->redo
        // round trip `inv-undo-redo-reference-heal` reads — see
        // `crate::pbt::undo_redo_density`.
        Redo.preconditions(state)
            .map(|()| (crate::pbt::undo_redo_density::weight(2), Just(Redo).boxed()))
    }
}

impl<R: RefLifecycle + RefBlockTreeMut> TransitionRef<R> for Redo {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.has_redo_history(), Reason::NoRedoHistory),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Pop redo→undo and reset every region cursor — the whole effect lives
        // in the ref cap.
        state.redo_last_and_reset_cursors();
    }
}

crate::cap_transition! {
    Redo: SutHistoryWrite,
    where R: [ RefLifecycle ],
    |_me, _state, sut| {
        sut.redo().await;
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

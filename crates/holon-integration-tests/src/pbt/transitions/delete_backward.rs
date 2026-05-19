//! Transition: delete characters backward in the active editor.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1664-1680` (generator),
//! `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2966-2969` (ref-state apply),
//! `sut.rs:4420-4429` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_pbt_core::capabilities::{
    CapRegion, RefBlockTreeMut, RefEditorMirror, RefEditorMirrorMut, RefFocus, RefLifecycle,
    commit_active_editor_if_changed,
};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

/// Delete `count` characters backward in the active editor.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct DeleteBackward {
    pub count: usize,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn delete_backward_preconditions<R: RefEditorMirror + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(), Reason> {
    let in_memory_len = state.active_editor_text().map(|t| t.len()).unwrap_or(0);
    let checks: Vec<Validated<(), Reason>> = vec![
        check(R::atomic_editor_enabled(), Reason::AtomicEditorDisabled),
        check(state.enable_loro(), Reason::LoroRequiredForAtomicEditor),
        check(state.app_started(), Reason::AppNotStarted),
        check(state.is_properly_setup(), Reason::NotProperlySetup),
        check(
            state.current_focus(CapRegion::Main).is_some(),
            Reason::NoFocusInMain,
        ),
        check(
            state.active_editor_block().is_some(),
            Reason::NoActiveEditor,
        ),
        check(in_memory_len > 0, Reason::EditorContentEmpty),
    ];
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

pub fn delete_backward_weighted_generator<R: RefEditorMirror + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<DeleteBackward>), Reason> {
    delete_backward_preconditions(state).map(|_| {
        let in_memory_len = state.active_editor_text().map(|t| t.len()).unwrap_or(0);
        let last = state.last_transition_kind();
        let db_weight = match last {
            Some("TypeChars") => 5,
            Some("FocusEditableText") if in_memory_len > 0 => 4,
            _ => 1,
        };
        let max_delete = in_memory_len.min(4);
        let strat = (1usize..=max_delete)
            .prop_map(|count| DeleteBackward { count })
            .boxed();
        (db_weight, strat)
    })
}

pub fn delete_backward_apply_to_ref<R>(count: usize, state: &mut R)
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
{
    state.delete_backward(count);
    // Same Phase 2 contract as TypeChars: per-keystroke writes flow
    // through MutableText → Loro → SQL between transitions. The CDC
    // quiescence barrier in the PBT runner means block.content has
    // settled to the typed-or-trimmed form by the next invariant
    // check, so the ref model must commit too.
    if state.enable_loro() {
        commit_active_editor_if_changed(state);
    }
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl E2ETransitionFactory for DeleteBackward {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        delete_backward_weighted_generator(state)
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for DeleteBackward {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        delete_backward_preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        delete_backward_apply_to_ref(self.count, state);
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_delete_backward(self.count).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

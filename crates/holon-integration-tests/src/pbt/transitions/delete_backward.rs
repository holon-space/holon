//! Transition: delete characters backward in the active editor.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1664-1680` (generator),
//! `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2966-2969` (ref-state apply),
//! `sut.rs:4420-4429` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use holon_pbt_core::capabilities::{
    CapRegion, RefBlockTreeMut, RefEditorMirror, RefEditorMirrorMut, RefFocus, RefFocusMut,
    RefLifecycle, SutEditorMirrorWrite, commit_active_editor_if_changed,
};
use holon_pbt_core::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use holon_pbt_core::{TransitionFactory, TransitionRef};

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
        check(state.has_editor_buffer(), Reason::NoEditorBuffer),
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
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefFocusMut + RefLifecycle,
{
    // Per-keystroke, exactly like the SUT's `HeadlessEditorMirror` routes
    // each raw `backspace`: at cursor 0 it dispatches the STRUCTURAL
    // `join_block` (same as PressKey's Backspace-at-0 arm); mid-line it
    // deletes one char. Modeling the whole `count` as a flat char delete
    // missed the join — the SUT merged blocks while the ref only trimmed
    // text (the Full-slice SplitBlock→DeleteBackward divergence).
    for _ in 0..count {
        let cursor = state.active_editor_cursor().unwrap_or(0);
        if cursor > 0 {
            state.delete_backward(1);
            continue;
        }
        let Some(block_id) = state.active_editor_block() else {
            break;
        };
        // Backspace at 0: commit pending edits, then join — IF a merge
        // target exists (mirrors press_key.rs; prod's `join_block` op
        // no-ops without one, so the ref no-ops too and the next
        // backspace hits the same wall).
        commit_active_editor_if_changed(state);
        let prev = state.previous_sibling(&block_id);
        let target = match (&prev, state.parent_of(&block_id)) {
            (Some(p), _) => Some(p.clone()),
            (None, Some(parent)) => (state.is_text_block(&parent)
                && !state.is_layout_block(&parent)
                && !parent.is_no_parent()
                && !parent.is_sentinel())
            .then_some(parent),
            (None, None) => None,
        };
        let Some(target) = target else {
            continue;
        };
        // Join boundary = the merge target's pre-join content length —
        // prod returns it in the op response and the (now seed-aware)
        // headless mirror adopts it for the next keystroke.
        let boundary = state.block_content(&target).map(str::len).unwrap_or(0);
        crate::pbt::transitions::join_block::join_block_apply_to_ref(&block_id, state);
        let joined = state
            .block_content(&target)
            .map(str::to_owned)
            .unwrap_or_default();
        state.open_active_editor(target, joined, boundary);
    }
    // Same Phase 2 contract as TypeChars: per-keystroke writes flow
    // through MutableText → Loro → SQL between transitions. The CDC
    // quiescence barrier in the PBT runner means block.content has
    // settled to the typed-or-trimmed form by the next invariant
    // check, so the ref model must commit too.
    //
    // Under SqlOnly (no cell attached) post-join backspaces stay pending
    // in the editor until blur — do NOT commit here. Pending text DOES
    // commit at the next structural op ("structural ops are commit
    // points", docs/Architecture/UI.md — split/join/indent flush first,
    // both in prod and in the ref applies), so the only remaining
    // pending-vs-committed divergence window is mid-transition. CAUTION:
    // The GPUI editor now always commits typed text (see TypeChars fix).
    commit_active_editor_if_changed(state);
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl<R: RefEditorMirror + RefFocus + RefLifecycle> TransitionFactory<R> for DeleteBackward {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // ADR 0009 asymmetry #1 (same as TypeChars/PressKey): editor primitives
        // work on any block store, so gate to AnyStorageOf({Loro, Turso}) —
        // without this, `active_editor` is never set under Turso-only and the
        // widened TypeChars/PressKey gates are unreachable in exactly the
        // configuration they were widened for.
        ::holon_pbt_core::RequiredWiring::any_storage_of([
            ::holon_pbt_core::StorageAdapter::Loro,
            ::holon_pbt_core::StorageAdapter::Turso,
        ])
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        delete_backward_weighted_generator(state)
    }
}

impl<
    R: RefEditorMirror + RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefFocusMut + RefLifecycle,
> TransitionRef<R> for DeleteBackward
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        delete_backward_preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        delete_backward_apply_to_ref(self.count, state);
    }
}

crate::cap_transition! {
    DeleteBackward: SutEditorMirrorWrite,
    where R: [ RefEditorMirror + RefFocus + RefLifecycle ],
    |me, _state, sut| {
        sut.apply_delete_backward(me.count).await;
    }
    sql_budget: |_me, _state| {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

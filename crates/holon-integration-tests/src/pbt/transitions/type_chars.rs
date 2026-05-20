//! Transition: type characters into the active editor.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1650-1662` (generator),
//! `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2961-2964` (ref-state apply),
//! `sut.rs:4409-4418` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_pbt_core::capabilities::{
    CapRegion, RefBlockTreeMut, RefEditorMirror, RefEditorMirrorMut, RefFocus, RefLifecycle,
    SutEditorMirrorWrite, commit_active_editor_if_changed,
};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

/// Type a short ASCII string into the active editor.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct TypeChars {
    pub text: String,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────
//
// These are the canonical logic; the `TransitionImpl` below just delegates.
// The pure slice can call these directly without `TransitionImpl`.

/// Preconditions for `TypeChars`, bound only on the capability traits it reads.
pub fn type_chars_preconditions<R: RefEditorMirror + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(), Reason> {
    let checks: Vec<Validated<(), Reason>> = vec![
        check(R::atomic_editor_enabled(), Reason::AtomicEditorDisabled),
        check(
            state.enable_loro() || ReferenceState::real_editor_enabled(),
            Reason::LoroRequiredForAtomicEditor,
        ),
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
    ];
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

/// Weighted generator for `TypeChars`, capability-bound.
pub fn type_chars_weighted_generator<R: RefEditorMirror + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<TypeChars>), Reason> {
    type_chars_preconditions(state).map(|_| {
        let last = state.last_transition_kind();
        let tc_weight = match last {
            Some("FocusEditableText") | Some("MoveCursor") => 6,
            Some("TypeChars") => 4,
            _ => 1,
        };
        let strat = crate::pbt::generators::typing_text_strategy()
            .prop_map(|text: String| TypeChars { text })
            .boxed();
        (tc_weight, strat)
    })
}

/// Ref-state apply for `TypeChars`, capability-bound. Mirrors the
/// original ReferenceState-specific apply exactly: type into the active
/// editor, then commit through to block content if Loro is enabled.
pub fn type_chars_apply_to_ref<R>(text: &str, state: &mut R)
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
{
    state.type_chars(text);
    // After Phase 1 of `devlog/2026-05-08-154449-split-block-discards-pending-edits.md`:
    // when Loro is enabled, per-keystroke writes flow through
    // `MutableText` into the global Loro doc, and `LoroSyncController`
    // projects them into `block.content` SQL between transitions (CDC
    // quiescence barrier at the PBT runner). SqlOnly has no Loro path —
    // typing only lives in the editor's `InputState` until on-blur
    // fires `set_field`, so ref-state shouldn't commit either.
    if state.enable_loro() {
        commit_active_editor_if_changed(state);
    }
}

// ── E2E trait impls (wide PBT entry point; delegate to _cap fns) ──

impl<R: RefEditorMirror + RefFocus + RefLifecycle> TransitionFactory<R> for TypeChars {
    type Reason = Reason;
    fn required_wiring() -> ::holon_pbt_core::RequiredWiring {
        // ADR 0009 asymmetry #1: "edit content" works on any block store (under
        // Turso-only via the on-blur `set_field` path), so gate to
        // `AnyStorageOf({Loro, Turso})` — bisectable across the storage axis.
        // Headless Turso-only slices stay unaffected: `preconditions` still
        // requires `enable_loro() || real_editor_enabled()`.
        ::holon_pbt_core::RequiredWiring::any_storage_of([
            ::holon_pbt_core::StorageAdapter::Loro,
            ::holon_pbt_core::StorageAdapter::Turso,
        ])
    }

    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        type_chars_weighted_generator(state)
    }
}

impl<R: RefEditorMirror + RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle>
    TransitionRef<R> for TypeChars
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        type_chars_preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        type_chars_apply_to_ref(&self.text, state);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutEditorMirrorWrite> TransitionImpl<ReferenceState, S> for TypeChars {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.apply_type_chars(&self.text).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for TypeChars {
    fn expected_sql(&self, _: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

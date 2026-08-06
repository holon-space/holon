//! Transition: type characters into the active editor.
//!
//! @pbt rung input-pipeline
//!   `apply_type_chars` drives editor keystrokes through the production
//!   ReactiveEngineDriver -> HeadlessEditorMirror.
//! @pbt covers editor-typing — character keystrokes -> MutableText edit
//!
//! Mirrors the legacy logic split across `state_machine.rs:1650-1662`
//! (generator), `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2961-2964` (ref-state apply),
//! `sut.rs:4409-4418` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutEditorMirrorWrite;
use holon_pbt_core::capabilities::commit_active_editor_if_changed;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;

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
/// original reference-state-specific apply exactly: type into the active
/// editor, then commit through to block content.
pub fn type_chars_apply_to_ref<R>(text: &str, state: &mut R)
where
    R: RefEditorMirrorMut + RefBlockTreeMut + RefFocus + RefLifecycle,
{
    state.type_chars(text);
    // The GPUI editor now always commits typed text: when Loro is
    // enabled the per-keystroke pipeline writes through the Cell into
    // the Loro doc, and when no cell is attached (SqlOnly / no-Loro
    // mode) the change handler falls back to `set_field("content")`.
    // The ref must mirror this so the invariant sees the same content
    // on both sides regardless of storage backend.
    commit_active_editor_if_changed(state);
}

// ── E2E trait impls (wide PBT entry point; delegate to _cap fns) ──

impl<R: RefEditorMirror + RefFocus + RefLifecycle> TransitionFactory<R> for TypeChars {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

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

crate::cap_transition! {
    TypeChars: SutEditorMirrorWrite,
    where R: [ RefEditorMirror + RefFocus + RefLifecycle ],
    |me, _state, sut| {
        sut.apply_type_chars(&me.text).await;
    }
    sql_budget: |_me, _state| {
        // Bimodal over 6 dedup samples (d=3): 7 when the keystroke only
        // touches the editor buffer (n=3), 15 when it commits and the write
        // reaches storage (n=3) — `backend.execute_operation ▸
        // dispatcher.execute_operation` plus `home.locate`
        // (+ `home.resolve_doc`), `home.prev_sibling` and
        // `org.on_block_feed ▸ org.on_block_changed`. The committing mode is
        // the budget; `REACTIVE_BASE` alone never covered it.
        ExpectedSql {
            reads: REACTIVE_BASE + 10,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

//! Transition: move the active editor's caret to a byte position.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1637-1648`
//! (generator), `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2956-2959` (ref-state apply),
//! `sut.rs:4395-4408` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefEditorMirror;
use holon_pbt_core::capabilities::RefEditorMirrorMut;
use holon_pbt_core::capabilities::RefFocus;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutEditorMirrorWrite;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;

/// Move the active editor caret to a given byte position.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct MoveCursor {
    pub byte_position: usize,
}

// ── Capability-bound free functions (Phase 3) ─────────────────────

pub fn move_cursor_preconditions<R: RefEditorMirror + RefFocus + RefLifecycle>(
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

pub fn move_cursor_weighted_generator<R: RefEditorMirror + RefFocus + RefLifecycle>(
    state: &R,
) -> Validated<(u32, BoxedStrategy<MoveCursor>), Reason> {
    move_cursor_preconditions(state).map(|_| {
        // Char-boundary byte offsets only — a caret can't sit mid-codepoint,
        // and both the ref mirror and prod slice at this offset.
        let boundaries: Vec<usize> = state
            .active_editor_text()
            .map(|t| {
                t.char_indices()
                    .map(|(i, _)| i)
                    .chain(std::iter::once(t.len()))
                    .collect()
            })
            .unwrap_or_else(|| vec![0]);
        let last = state.last_transition_kind();
        let mc_weight = match last {
            Some("FocusEditableText") => 4,
            _ => 1,
        };
        let strat = prop::sample::select(boundaries)
            .prop_map(|byte_position| MoveCursor { byte_position })
            .boxed();
        (mc_weight, strat)
    })
}

pub fn move_cursor_apply_to_ref<R: RefEditorMirrorMut>(byte_position: usize, state: &mut R) {
    state.move_cursor(byte_position);
}

// ── E2E trait impls (delegate to _cap fns) ────────────────────────

impl<R: RefEditorMirror + RefFocus + RefLifecycle> TransitionFactory<R> for MoveCursor {
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
        move_cursor_weighted_generator(state)
    }
}

impl<R: RefEditorMirror + RefEditorMirrorMut + RefFocus + RefLifecycle> TransitionRef<R>
    for MoveCursor
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        move_cursor_preconditions(state)
    }

    fn apply_to_ref(&self, state: &mut R) {
        move_cursor_apply_to_ref(self.byte_position, state);
    }
}

crate::cap_transition! {
    MoveCursor: SutEditorMirrorWrite,
    where R: [ RefEditorMirror + RefFocus + RefLifecycle ],
    |me, _state, sut| {
        sut.apply_move_cursor(me.byte_position).await;
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

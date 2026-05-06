//! Transition: type characters into the active editor.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1650-1662` (generator),
//! `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2961-2964` (ref-state apply),
//! `sut.rs:4409-4418` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::Region;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

/// Type a short ASCII string into the active editor.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug)]
pub struct TypeChars {
    pub text: String,
}

impl E2ETransitionFactory for TypeChars {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Strategy generates random 1-4 character strings; let `preconditions`
        // validate all state constraints (atomic_editor, loro, editor setup, focus).
        // Delegate to a probe instance to check if TypeChars can succeed.
        let probe = TypeChars {
            text: String::new(),
        };
        probe.preconditions(state).map(|_| {
            let last = state.last_transition_kind;
            let tc_weight = match last {
                Some("FocusEditableText") | Some("MoveCursor") => 6,
                Some("TypeChars") => 4,
                _ => 1,
            };

            let strat = "[a-z]{1,4}"
                .prop_map(|text: String| TypeChars { text })
                .boxed();
            (tc_weight, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for TypeChars {
    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(
                ReferenceState::atomic_editor_enabled(),
                Reason::AtomicEditorDisabled,
            ),
            check(
                state.variant.enable_loro,
                Reason::LoroRequiredForAtomicEditor,
            ),
            check(state.app_started, Reason::AppNotStarted),
            check(state.is_properly_setup(), Reason::NotProperlySetup),
            check(
                state.current_focus(Region::Main).is_some(),
                Reason::NoFocusInMain,
            ),
            check(state.active_editor.is_some(), Reason::NoActiveEditor),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if let Some(editor) = state.active_editor.as_mut() {
            editor.type_chars(&self.text);
        }
        // After Phase 1 of `devlog/2026-05-08-154449-split-block-discards-pending-edits.md`:
        // when Loro is enabled, per-keystroke writes flow through
        // `MutableText` into the global Loro doc, and
        // `LoroSyncController` projects them into `block.content` SQL
        // between transitions (CDC quiescence barrier at the PBT runner).
        // SqlOnly has no Loro path — typing only lives in the editor's
        // `InputState` until on-blur fires `set_field`, so ref-state
        // shouldn't commit either.
        if state.variant.enable_loro {
            state.commit_active_editor_if_changed();
        }
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_type_chars(&self.text).await;
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

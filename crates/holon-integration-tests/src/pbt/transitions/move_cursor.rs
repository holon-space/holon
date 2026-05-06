//! Transition: move the active editor's caret to a byte position.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1637-1648` (generator),
//! `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2956-2959` (ref-state apply),
//! `sut.rs:4395-4408` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

/// Move the active editor caret to a given byte position.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug)]
pub struct MoveCursor {
    pub byte_position: usize,
}

impl E2ETransitionFactory for MoveCursor {
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Check preconditions via a dummy instance. All bytes in [0, len] are
        // valid per preconditions; actual position is random within that range.
        let dummy = MoveCursor { byte_position: 0 };
        dummy.preconditions(state).map(|_| {
            let in_memory_len = state
                .active_editor
                .as_ref()
                .map(|e| e.in_memory_content.len())
                .unwrap_or(0);

            let last = state.last_transition_kind;
            let mc_weight = match last {
                Some("FocusEditableText") => 4,
                _ => 1,
            };

            let strat = (0..=in_memory_len)
                .prop_map(|byte_position| MoveCursor { byte_position })
                .boxed();
            (mc_weight, strat)
        })
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for MoveCursor {
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
                state.current_focus(holon_api::Region::Main).is_some(),
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
            editor.move_cursor(self.byte_position);
        }
    }

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_move_cursor(self.byte_position).await;
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

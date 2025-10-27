//! Transition: move the active editor's caret to a byte position.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1637-1648` (generator),
//! `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2956-2959` (ref-state apply),
//! `sut.rs:4395-4408` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

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
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !ReferenceState::atomic_editor_enabled() {
            return None;
        }
        if !state.app_started {
            return None;
        }
        if !state.is_properly_setup() {
            return None;
        }
        if state.current_focus(holon_api::Region::Main).is_none() {
            return None;
        }
        if state.active_editor.is_none() {
            return None;
        }

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
        Some((mc_weight, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for MoveCursor {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        ReferenceState::atomic_editor_enabled() && state.active_editor.is_some()
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if let Some(editor) = state.active_editor.as_mut() {
            editor.move_cursor(self.byte_position);
        }
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_move_cursor(self.byte_position).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, _state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE,
            writes: 0,
            ddl: 0,
            tolerance: 5,
        }
    }
}

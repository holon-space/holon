//! Transition: delete characters backward in the active editor.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1664-1680` (generator),
//! `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2966-2969` (ref-state apply),
//! `sut.rs:4420-4429` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

/// Delete `count` characters backward in the active editor.
/// Gated to `PBT_ATOMIC_EDITOR=1` runs.
#[derive(Clone, Debug)]
pub struct DeleteBackward {
    pub count: usize,
}

impl E2ETransitionFactory for DeleteBackward {
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
        let in_memory_len = state
            .active_editor
            .as_ref()
            .map(|e| e.in_memory_content.len())
            .unwrap_or(0);
        if in_memory_len == 0 {
            return None;
        }

        let last = state.last_transition_kind;
        let db_weight = match last {
            Some("TypeChars") => 5,
            Some("FocusEditableText") if in_memory_len > 0 => 4,
            _ => 1,
        };

        let max_delete = in_memory_len.min(4);
        let strat = (1usize..=max_delete)
            .prop_map(|count| DeleteBackward { count })
            .boxed();
        Some((db_weight, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for DeleteBackward {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        ReferenceState::atomic_editor_enabled() && state.active_editor.is_some()
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if let Some(editor) = state.active_editor.as_mut() {
            editor.delete_backward(self.count);
        }
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_delete_backward(self.count).await;
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

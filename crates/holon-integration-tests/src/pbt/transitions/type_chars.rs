//! Transition: type characters into the active editor.
//!
//! Mirrors the legacy logic split across `state_machine.rs:1650-1662` (generator),
//! `state_machine.rs:3552-3556` (precondition, shared arm),
//! `state_machine.rs:2961-2964` (ref-state apply),
//! `sut.rs:4409-4418` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

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

        let last = state.last_transition_kind;
        let tc_weight = match last {
            Some("FocusEditableText") | Some("MoveCursor") => 6,
            Some("TypeChars") => 4,
            _ => 1,
        };

        let strat = "[a-z]{1,4}"
            .prop_map(|text: String| TypeChars { text })
            .boxed();
        Some((tc_weight, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for TypeChars {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        ReferenceState::atomic_editor_enabled() && state.active_editor.is_some()
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if let Some(editor) = state.active_editor.as_mut() {
            editor.type_chars(&self.text);
        }
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_type_chars(&self.text).await;
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

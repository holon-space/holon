//! Transition: blur the currently active editor (Escape key).
//!
//! Mirrors the legacy logic split across `state_machine.rs:1719-1720` (generator),
//! `state_machine.rs:3552-3556` (precondition),
//! `state_machine.rs:2971-2974` (ref-state apply),
//! `sut.rs:4464-4471` (SUT apply), and
//! `transition_budgets.rs:368-377` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE};

/// Blur the currently active editor by sending Escape.
#[derive(Clone, Debug)]
pub struct Blur;

impl E2ETransitionFactory for Blur {
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
        Some((1, Just(Blur).boxed()))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for Blur {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        ReferenceState::atomic_editor_enabled() && state.active_editor.is_some()
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.commit_active_editor_if_changed();
        state.active_editor = None;
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_blur().await;
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

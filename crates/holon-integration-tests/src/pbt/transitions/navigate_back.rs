//! Transition: navigate back in the per-region navigation history.
//!
//! Pilot variant for the file-per-transition refactor. Mirrors the
//! legacy logic split across `state_machine.rs:603-610` (generator),
//! `state_machine.rs:3168-3170` (precondition),
//! `state_machine.rs:2242-2250` (ref-state apply),
//! `sut.rs:1016-1028` (SUT apply), and
//! `transition_budgets.rs:165-172` (expected SQL).

use holon_api::Region;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Pop one entry off the active navigation history for `region`.
/// Mirrors what production's history `Back` button does. The reference
/// model also clears per-region focus to match how the SUT lets engine
/// focus drift to whatever last touched it.
#[derive(Clone, Debug)]
pub struct NavigateBack {
    pub region: Region,
}

impl E2ETransitionFactory for NavigateBack {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        // Restricted to Main — only TUI binding (leader+'b') targets
        // `region: "main"`. See `assets/default/keybindings.yaml`.
        if !state.can_go_back(Region::Main) {
            return None;
        }
        let strat = proptest::strategy::Just(NavigateBack {
            region: Region::Main,
        })
        .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for NavigateBack {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started && state.can_go_back(self.region)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        if let Some(history) = state.navigation_history.get_mut(&self.region)
            && history.cursor > 0
        {
            history.cursor -= 1;
        }
        state.focused_entity_id.remove(&self.region);
        state.focused_cursor.remove(&self.region);
    }

    async fn apply_to_sut(&self, _state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.navigate_back(self.region).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS - 2,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use holon_api::{EntityUri, Region};

    use super::*;
    use crate::pbt::reference_state::{NavigationHistory, ReferenceState};
    use crate::pbt::transitions::E2ETransition;
    use crate::pbt::types::TestVariant;

    fn make_state_with_back_history() -> ReferenceState {
        let interp = Arc::new(holon_frontend::render_interpreter::RenderInterpreter::new());
        let mut state = ReferenceState::new(TestVariant::full(), interp);
        state.app_started = true;
        let mut history = NavigationHistory::new();
        history
            .entries
            .push(Some(EntityUri::block("block:test-target")));
        history.cursor = 1;
        state.navigation_history.insert(Region::Main, history);
        state
            .focused_entity_id
            .insert(Region::Main, EntityUri::block("block:test-target"));
        state
    }

    /// Pilot validation: the wrapping struct's direct trait calls and
    /// the macro-generated enum dispatch produce the same effect.
    #[test]
    fn navigate_back_apply_via_struct_and_enum_match() {
        let nb = NavigateBack {
            region: Region::Main,
        };

        // 1. Direct call on the struct.
        let mut state_a = make_state_with_back_history();
        assert!(nb.preconditions(&state_a));
        nb.apply_to_ref(&mut state_a);
        assert_eq!(
            state_a.navigation_history[&Region::Main].cursor,
            0,
            "cursor should decrement"
        );
        assert!(
            !state_a.focused_entity_id.contains_key(&Region::Main),
            "per-region focus should be cleared"
        );

        // 2. Same call routed through the macro-generated enum.
        let wrapped: E2ETransition = NavigateBack {
            region: Region::Main,
        }
        .into();
        let mut state_b = make_state_with_back_history();
        assert!(wrapped.preconditions(&state_b));
        wrapped.apply_to_ref(&mut state_b);
        assert_eq!(state_b.navigation_history[&Region::Main].cursor, 0);
        assert!(!state_b.focused_entity_id.contains_key(&Region::Main));
    }

    /// Pilot validation: factory returns Some when applicable, None
    /// before app start.
    #[test]
    fn navigate_back_factory_state_gating() {
        let interp = Arc::new(holon_frontend::render_interpreter::RenderInterpreter::new());
        let mut pre_start = ReferenceState::new(TestVariant::full(), interp);
        // app_started == false — must skip.
        assert!(NavigateBack::weighted_generator(&pre_start).is_none());

        // Even with app_started, no nav history → still skip.
        pre_start.app_started = true;
        assert!(NavigateBack::weighted_generator(&pre_start).is_none());

        // With back-history present → factory yields a strategy.
        let with_back = make_state_with_back_history();
        assert!(NavigateBack::weighted_generator(&with_back).is_some());
    }
}

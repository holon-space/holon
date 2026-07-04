//! Transition: navigate back in the per-region navigation history.
//!
//! Pilot variant for the file-per-transition refactor. Mirrors the
//! legacy logic split across `state_machine.rs:603-610` (generator),
//! `state_machine.rs:3168-3170` (precondition),
//! `state_machine.rs:2242-2250` (ref-state apply),
//! `sut.rs:1016-1028` (SUT apply), and
//! `transition_budgets.rs:165-172` (expected SQL).

use holon_api::Region;
use holon_pbt_core::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use holon_pbt_core::capabilities::{RefLifecycle, RefNavHistoryMut, SutNavHistoryDrive};
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Pop one entry off the active navigation history for `region`.
/// Mirrors what production's history `Back` button does. The reference
/// model also clears per-region focus to match how the SUT lets engine
/// focus drift to whatever last touched it.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NavigateBack {
    pub region: Region,
}

impl<R: RefLifecycle + RefNavHistoryMut> TransitionFactory<R> for NavigateBack {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let instance = NavigateBack {
            region: Region::Main,
        };
        instance.preconditions(state).map(|()| {
            let strat = proptest::strategy::Just(instance).boxed();
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefNavHistoryMut> TransitionRef<R> for NavigateBack {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.can_go_back(self.region), Reason::NoNavigationHistory),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        state.nav_step_back(self.region);
    }
}

crate::cap_transition! {
    NavigateBack: SutNavHistoryDrive,
    where R: [ RefLifecycle + RefNavHistoryMut ],
    |me, _state, sut| {
        sut.navigate_back(me.region).await;
    }
    sql_budget: |_me, state| {
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

    fn make_state_with_back_history() -> ReferenceState {
        let interp = Arc::new(holon_frontend::render_interpreter::RenderInterpreter::new());
        let mut state = ReferenceState::new(holon_pbt_core::Wiring::full(), interp);
        state.action.app_started = true;
        let mut history = NavigationHistory::new();
        history.entries.push(Some(EntityUri::block("test-target")));
        history.cursor = 1;
        state
            .ui
            .tab
            .navigation_history
            .insert(Region::Main, history);
        state
            .ui
            .tab
            .focused_entity_id
            .insert(Region::Main, EntityUri::block("test-target"));
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
        assert!(nb.preconditions(&state_a).is_good());
        nb.apply_to_ref(&mut state_a);
        assert_eq!(
            state_a.ui.tab.navigation_history[&Region::Main].cursor,
            0,
            "cursor should decrement"
        );
        assert!(
            !state_a.ui.tab.focused_entity_id.contains_key(&Region::Main),
            "per-region focus should be cleared"
        );

        // 2. Same call routed through the macro-generated enum.
        let wrapped: E2ETransition = NavigateBack {
            region: Region::Main,
        }
        .into();
        let mut state_b = make_state_with_back_history();
        assert!(wrapped.preconditions(&state_b).is_good());
        wrapped.apply_to_ref(&mut state_b);
        assert_eq!(state_b.ui.tab.navigation_history[&Region::Main].cursor, 0);
        assert!(!state_b.ui.tab.focused_entity_id.contains_key(&Region::Main));
    }

    /// Pilot validation: factory returns Good when applicable, Fail
    /// before app start.
    #[test]
    fn navigate_back_factory_state_gating() {
        let interp = Arc::new(holon_frontend::render_interpreter::RenderInterpreter::new());
        let mut pre_start = ReferenceState::new(holon_pbt_core::Wiring::full(), interp);
        // app_started == false — must skip.
        assert!(NavigateBack::weighted_generator(&pre_start).is_fail());

        // Even with app_started, no nav history → still skip.
        pre_start.action.app_started = true;
        assert!(NavigateBack::weighted_generator(&pre_start).is_fail());

        // With back-history present → factory yields a strategy.
        let with_back = make_state_with_back_history();
        assert!(NavigateBack::weighted_generator(&with_back).is_good());
    }
}

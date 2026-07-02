//! Transition: navigate forward in the per-region navigation history.
//!
//! Mirrors the legacy logic split across `state_machine.rs:612-619` (generator),
//! `state_machine.rs:3171-3173` (precondition),
//! `state_machine.rs:2251-2259` (ref-state apply),
//! `sut.rs:1308-1314` (SUT apply), and
//! `transition_budgets.rs:176-181` (expected SQL).

use crate::pbt::validation::{Reason, check};
use holon_api::Region;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::capabilities::{
    RefFocusMut, RefLifecycle, RefNavHistory, RefNavHistoryMut, SutNavHistoryDrive,
};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    ExpectedSql, JOURNAL_READS, NAV_DML_READS, REACTIVE_BASE, docs_tolerance,
};

/// Pop one entry forward in the active navigation history for `region`.
/// Mirrors the forward-button in production's per-region history stack.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct NavigateForward {
    pub region: Region,
}

impl TransitionFactory<ReferenceState> for NavigateForward {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutNavHistoryDrive,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Restricted to Main — only TUI binding (leader+'f') targets
        // `region: "main"`. See `assets/default/keybindings.yaml`.
        let instance = NavigateForward {
            region: Region::Main,
        };
        instance.preconditions(state).map(|_| {
            let strat = proptest::strategy::Just(instance).boxed();
            (1, strat)
        })
    }
}

fn navigate_forward_preconditions<R: RefLifecycle + RefNavHistory>(
    region: Region,
    state: &R,
) -> Validated<(), Reason> {
    let checks: Vec<Validated<(), Reason>> = vec![
        check(state.app_started(), Reason::AppNotStarted),
        check(state.can_go_forward(region), Reason::NoNavigationHistory),
    ];
    checks
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(|_| ())
}

fn navigate_forward_apply_to_ref<R: RefNavHistoryMut + RefFocusMut>(region: Region, state: &mut R) {
    state.nav_history_forward(region);
    state.clear_region_focus(region);

    // Blur on nav: see `navigate_focus.rs` for verification.
    state.blur_active_editor();
}

impl TransitionRef<ReferenceState> for NavigateForward {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        navigate_forward_preconditions(self.region, state)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        navigate_forward_apply_to_ref(self.region, state);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutNavHistoryDrive> TransitionImpl<ReferenceState, S> for NavigateForward {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.navigate_forward(self.region).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for NavigateForward {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS - 2,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

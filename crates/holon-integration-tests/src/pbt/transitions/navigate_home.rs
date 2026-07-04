//! Transition: navigate home (return to root) in a region.
//!
//! @pbt rung dispatch
//!   UNFAITHFUL SHORTCUT (audit TR-NAV): `apply_navigate_home` dispatches
//!   `navigation.go_home` directly, bypassing the leader-h chord path (the
//!   same op the GPUI/CLI leader-h dispatches).
//! @pbt covers nav-home — return-to-root in a region (op-level only)
//!
//! Mirrors the legacy logic split across `state_machine.rs:621-626`
//! (generator), `state_machine.rs:3174` (precondition),
//! `state_machine.rs:2260-2276` (ref-state apply),
//! `sut.rs:1316-1330` (SUT apply), and
//! `transition_budgets.rs:182-187` (expected SQL).

use holon_api::Region;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefNavHistoryMut;
use holon_pbt_core::capabilities::SutNavHistoryWrite;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::JOURNAL_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::NAV_DML_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Return to root (home) in a region's navigation history.
/// Clears all navigation state for the region and sets focus to None globally.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I navigate home in region {region}")]
pub struct NavigateHome {
    pub region: Region,
}

impl<R: RefLifecycle + RefNavHistoryMut> TransitionFactory<R> for NavigateHome {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Restricted to Main: the only TUI binding for `go_home` is
        // leader+'h' which always targets `region: "main"`. See
        // `assets/default/keybindings.yaml`. The previous
        // generator emitted all three regions and the SUT covered the
        // gap with a `execute_op + manual set_focus` shortcut — that's
        // item A3 in `frontends/tui/TODO.md` removes.
        let instance = NavigateHome {
            region: Region::Main,
        };
        instance.preconditions(state).map(|_| {
            let strat = proptest::strategy::Just(instance).boxed();
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefNavHistoryMut> TransitionRef<R> for NavigateHome {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started(), Reason::AppNotStarted)];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole `focus(region, None)` reference effect — idempotency guard,
        // home-row push, open-pin reset, region + global focus clear, editor blur
        // — lives in `RefNavHistoryMut::nav_go_home` (mirrors `provider.rs::focus`).
        state.nav_go_home(self.region);
    }
}

crate::cap_transition! {
    NavigateHome: SutNavHistoryWrite,
    where R: [ RefLifecycle + RefNavHistoryMut ],
    |me, _state, sut| {
        // The generator only ever emits `Region::Main`; map it to the cap's
        // `CapRegion`. Any other region is a generator bug, not a runtime case.
        let region = match me.region {
            Region::Main => CapRegion::Main,
            other => panic!("NavigateHome generator must only emit Main; got {other:?}"),
        };
        sut.apply_navigate_home(region).await;
    }
    sql_budget: |_me, state| {
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state),
        }
    }
}

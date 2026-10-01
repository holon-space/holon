//! Transition: resume every operation the engine's dispatch hold parked.
//!
//! @pbt rung dispatch
//!   waits until exactly `expect_parked` runs are parked at the top of
//!   `execute_operation`, then resumes them all and drops any hold no run
//!   matched.
//! @pbt covers dispatch-ordering-under-a-held-op — see `HoldDispatch`
//!
//! `expect_parked` makes a hold that never engaged a loud failure rather than
//! a vacuous pass. A leg that writes without the dispatcher (the Loro cell
//! leg's slot birth) parks nothing, and says so with `0`.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
#[cfg(feature = "otel-testing")]
use holon_pbt_core::capabilities::RefSqlCardinality;
use holon_pbt_core::capabilities::SutDispatchHold;
use holon_pbt_core::validation::Reason;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the {expect_parked} held dispatches are released")]
pub struct ReleaseHeld {
    pub expect_parked: usize,
}

impl<R> TransitionFactory<R> for ReleaseHeld {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(_: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

impl<R> TransitionRef<R> for ReleaseHeld {
    type Reason = Reason;

    fn preconditions(&self, _: &R) -> Validated<(), Reason> {
        Validated::Good(())
    }

    fn apply_to_ref(&self, _: &mut R) {}
}

crate::cap_transition! {
    ReleaseHeld: SutDispatchHold,
    |me, _state, sut| {
        sut.release_held_dispatches(me.expect_parked).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ReleaseHeld {
    fn expected_sql<R: RefSqlCardinality>(&self, state: &R) -> ExpectedSql {
        // Unmeasured: the resumed ops run in this window, so it carries their
        // writes and the reads they trigger.
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()),
            writes: 4,
            ddl: 0,
            tolerance: 20,
        }
    }
}

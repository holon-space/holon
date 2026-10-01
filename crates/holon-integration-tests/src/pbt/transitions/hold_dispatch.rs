//! Transition: park the next run of a named operation in the engine.
//!
//! @pbt rung dispatch
//!   arms the engine's test-only dispatch hold (`holon::api::dispatch_hold`);
//!   the op it names parks at the top of `execute_operation` until
//!   `ReleaseHeld`.
//! @pbt covers dispatch-ordering-under-a-held-op — an op that overlaps a held
//! op waits for it instead of overtaking it
//!
//! Hand-authored only: an order-correct SUT is invisible to a hold, so the
//! reference model is unchanged.

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
#[step_template("the next {entity}.{op} is held")]
pub struct HoldDispatch {
    pub entity: String,
    pub op: String,
}

impl<R> TransitionFactory<R> for HoldDispatch {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(_: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

impl<R> TransitionRef<R> for HoldDispatch {
    type Reason = Reason;

    fn preconditions(&self, _: &R) -> Validated<(), Reason> {
        Validated::Good(())
    }

    fn apply_to_ref(&self, _: &mut R) {}
}

crate::cap_transition! {
    HoldDispatch: SutDispatchHold,
    |me, _state, sut| {
        sut.hold_next_dispatch(&me.entity, &me.op);
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for HoldDispatch {
    fn expected_sql<R: RefSqlCardinality>(&self, state: &R) -> ExpectedSql {
        // Arming the hold reads nothing; the window carries the shared CDC
        // drain, as `Nothing` does.
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()),
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}

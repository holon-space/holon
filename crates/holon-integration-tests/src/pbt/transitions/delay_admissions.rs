//! Transition: delay the next admissions of a named operation in the engine.
//!
//! @pbt rung dispatch
//!   arms the engine's test-only admission delay (`holon::api::dispatch_hold`);
//!   each of the next `delays_ms.len()` admissions of the op blocks its thread
//!   for its delay. `ReleaseHeld` fails while a delay is unspent.
//! @pbt covers dispatch-ordering-admits-at-the-call — descending delays reverse
//! dispatches that admit on their spawned tasks, and leave dispatches that
//! admit at the call in order
//!
//! Hand-authored only: an order-correct SUT is invisible to a delay, so the
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
#[step_template("the next admissions of {entity}.{op} are delayed by {delays_ms} ms")]
pub struct DelayAdmissions {
    pub entity: String,
    pub op: String,
    pub delays_ms: Vec<u64>,
}

impl<R> TransitionFactory<R> for DelayAdmissions {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(_: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::HandAuthoredOnly)
    }
}

impl<R> TransitionRef<R> for DelayAdmissions {
    type Reason = Reason;

    fn preconditions(&self, _: &R) -> Validated<(), Reason> {
        Validated::Good(())
    }

    fn apply_to_ref(&self, _: &mut R) {}
}

crate::cap_transition! {
    DelayAdmissions: SutDispatchHold,
    |me, _state, sut| {
        sut.delay_next_admissions(&me.entity, &me.op, &me.delays_ms);
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for DelayAdmissions {
    fn expected_sql<R: RefSqlCardinality>(&self, state: &R) -> ExpectedSql {
        // Arming the delay reads nothing; the window carries the shared CDC
        // drain, as `Nothing` does.
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()),
            writes: 0,
            ddl: 0,
            tolerance: 0,
        }
    }
}

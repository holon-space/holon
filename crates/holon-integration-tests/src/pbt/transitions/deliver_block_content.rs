//! Transition: deliver async content into a deferred `live_block` placeholder.
//!
//! Only meaningful in the fast-UI layout PBT (where blueprints can mount
//! deferred placeholders). The integration-tests PBT runs a real backend
//! that always returns real data — no deferred placeholders to deliver.
//! So `weighted_generator` rejects unconditionally with a typed reason.

pub use holon_pbt_core::DeliverBlockContent;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;
use crate::pbt::validation::Reason;

impl TransitionFactory<ReferenceState> for DeliverBlockContent {
    type Reason = Reason;
    fn weighted_generator(_: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::DeliverNotMeaningfulInBackendTests)
    }
}

impl TransitionRef<ReferenceState> for DeliverBlockContent {
    type Reason = Reason;

    fn preconditions(&self, _: &ReferenceState) -> Validated<(), Reason> {
        Validated::fail(Reason::DeliverNotMeaningfulInBackendTests)
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {}
}

#[allow(async_fn_in_trait)]
impl<S> TransitionImpl<ReferenceState, S> for DeliverBlockContent {
    async fn apply_to_sut(&self, _: &ReferenceState, _: &mut S) {
        // Unreachable: `weighted_generator` and `preconditions` both hard-fail
        // (`DeliverNotMeaningfulInBackendTests`), so this variant is never
        // generated or applied. Fail loud if that ever changes.
        panic!(
            "[DeliverBlockContent] reached apply_to_sut for {} — backend PBT rejects \
             DeliverBlockContent in its generator/preconditions; the live-block delivery axis is \
             dead here.",
            self.block_id
        );
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for DeliverBlockContent {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5,
        }
    }
}

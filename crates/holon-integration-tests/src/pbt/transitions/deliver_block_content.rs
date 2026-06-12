//! Transition: deliver async content into a deferred `live_block` placeholder.
//!
//! Only meaningful in the fast-UI layout PBT (where blueprints can mount
//! deferred placeholders). The integration-tests PBT runs a real backend
//! that always returns real data — no deferred placeholders to deliver.
//! So `weighted_generator` rejects unconditionally with a typed reason.

use proptest::strategy::BoxedStrategy;
use validated::Validated;

pub use holon_pbt_core::DeliverBlockContent;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::validation::Reason;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

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
             DeliverBlockContent in its generator/preconditions; the live-block \
             delivery axis is dead here.",
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

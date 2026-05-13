//! Transition: deliver async content into a deferred `live_block` placeholder.
//!
//! Only meaningful in the fast-UI layout PBT (where blueprints can mount
//! deferred placeholders). The integration-tests PBT runs a real backend
//! that always returns real data — no deferred placeholders to deliver.
//! So `weighted_generator` rejects unconditionally with a typed reason.

use proptest::strategy::BoxedStrategy;
use validated::Validated;

pub use holon_pbt_core::DeliverBlockContent;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};
use crate::pbt::validation::Reason;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

impl E2ETransitionFactory for DeliverBlockContent {
    fn weighted_generator(_: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        Validated::fail(Reason::DeliverNotMeaningfulInBackendTests)
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for DeliverBlockContent {
    fn preconditions(&self, _: &ReferenceState) -> Validated<(), Reason> {
        Validated::fail(Reason::DeliverNotMeaningfulInBackendTests)
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {}

    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_deliver_block_content_loaded(&self.block_id).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5,
        }
    }
}

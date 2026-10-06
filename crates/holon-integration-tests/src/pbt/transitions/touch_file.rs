//! Transition: a no-op save gives a document's org file a new stamp over the
//! same bytes.
//!
//! @pbt rung external
//!   re-ticks the file in the in-memory vault and waits for the ingest.
//! @pbt covers write-back-state-machine — a new stamp without new bytes
//!   changes nothing in the store

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutWriteFaults;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::arm_write_churn::writeback_faults_enabled;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("a no-op save touches the file of document {doc}")]
pub struct TouchFile {
    pub doc: EntityUri,
}

impl TransitionFactory<ReferenceState> for TouchFile {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        if !writeback_faults_enabled() {
            return Validated::fail(Reason::HandAuthoredOnly);
        }
        let docs: Vec<EntityUri> = state.files.documents.keys().cloned().collect();
        check(
            state.action.app_started && !docs.is_empty(),
            Reason::NoDocumentsAvailable,
        )
        .map(|_| {
            (
                2,
                prop::sample::select(docs)
                    .prop_map(|doc| TouchFile { doc })
                    .boxed(),
            )
        })
    }
}

impl TransitionRef<ReferenceState> for TouchFile {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            state.action.app_started && state.files.documents.contains_key(&self.doc),
            Reason::NoDocumentsAvailable,
        )
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {}
}

crate::cap_transition! {
    TouchFile: SutWriteFaults,
    |me, _state, sut| {
        sut.touch_file(&me.doc).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for TouchFile {
    fn expected_sql<R: holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        state: &R,
    ) -> ExpectedSql {
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()),
            writes: 0,
            ddl: 0,
            tolerance: 40,
        }
    }
}

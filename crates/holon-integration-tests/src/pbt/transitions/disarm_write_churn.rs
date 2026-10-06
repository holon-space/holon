//! Transition: the process that kept rewriting a document's org file stops.
//!
//! @pbt rung external
//!   disarms the in-memory vault's churn on the file.
//! @pbt covers write-back-state-machine — once no file churns, every owed
//!   write-back has landed and no stall is disclosed any more

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
#[step_template("the process rewriting the file of document {doc} stops")]
pub struct DisarmWriteChurn {
    pub doc: EntityUri,
}

impl TransitionFactory<ReferenceState> for DisarmWriteChurn {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        if !writeback_faults_enabled() {
            return Validated::fail(Reason::HandAuthoredOnly);
        }
        let docs: Vec<EntityUri> = state.files.churning_docs().cloned().collect();
        check(!docs.is_empty(), Reason::NoDocumentsAvailable).map(|_| {
            (
                8,
                prop::sample::select(docs)
                    .prop_map(|doc| DisarmWriteChurn { doc })
                    .boxed(),
            )
        })
    }
}

impl TransitionRef<ReferenceState> for DisarmWriteChurn {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            state.files.churning_docs().any(|doc| *doc == self.doc),
            Reason::NoDocumentsAvailable,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.files.write_churn.remove(&self.doc);
    }
}

crate::cap_transition! {
    DisarmWriteChurn: SutWriteFaults,
    |me, _state, sut| {
        sut.disarm_write_churn(&me.doc).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for DisarmWriteChurn {
    fn expected_sql<R: holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        _: &R,
    ) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 8,
        }
    }
}

//! Transition: some process starts rewriting a document's org file with its
//! own bytes, so every write-back to it stalls.
//!
//! @pbt rung external
//!   arms the in-memory vault's churn on the file: each conditional write
//!   sees a new stamp over unchanged bytes.
//! @pbt covers write-back-state-machine — churn only delays a write-back; the
//!   end state after `DisarmWriteChurn` is the one without churn
//!
//! Generated only under `HOLON_PBT_WRITEBACK_FAULTS=1`.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutWriteFaults;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("some process starts rewriting the file of document {doc} with its own bytes")]
pub struct ArmWriteChurn {
    pub doc: EntityUri,
}

pub fn writeback_faults_enabled() -> bool {
    std::env::var("HOLON_PBT_WRITEBACK_FAULTS").is_ok_and(|v| v == "1")
}

fn candidates(state: &ReferenceState) -> Vec<EntityUri> {
    state
        .files
        .documents
        .keys()
        .filter(|doc| !state.files.write_churn.contains(*doc))
        .cloned()
        .collect()
}

impl TransitionFactory<ReferenceState> for ArmWriteChurn {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        if !writeback_faults_enabled() {
            return Validated::fail(Reason::HandAuthoredOnly);
        }
        let docs = candidates(state);
        check(
            state.action.app_started && !docs.is_empty(),
            Reason::NoDocumentsAvailable,
        )
        .map(|_| {
            (
                4,
                prop::sample::select(docs)
                    .prop_map(|doc| ArmWriteChurn { doc })
                    .boxed(),
            )
        })
    }
}

impl TransitionRef<ReferenceState> for ArmWriteChurn {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            state.action.app_started && candidates(state).contains(&self.doc),
            Reason::NoDocumentsAvailable,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.files.write_churn.insert(self.doc.clone());
        state.files.write_churn_ran = true;
    }
}

crate::cap_transition! {
    ArmWriteChurn: SutWriteFaults,
    |me, _state, sut| {
        sut.arm_write_churn(&me.doc).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ArmWriteChurn {
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

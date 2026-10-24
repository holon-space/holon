//! Transition: the disk starts refusing every write to a document's file, so
//! each write-back is attempted and fails.
//!
//! @pbt rung external
//!   arms the in-memory vault's write failure on the file: every write fails
//!   at its commit boundary, leaving the file's previous bytes complete.
//! @pbt covers write-back-state-machine — a refused write only leaves the file
//!   BEHIND its store (the same end state a churn delays), never ahead of it
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
#[step_template("the disk starts refusing every write to the file of document {doc}")]
pub struct ArmWriteFailure {
    pub doc: EntityUri,
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

impl TransitionFactory<ReferenceState> for ArmWriteFailure {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        if !super::arm_write_churn::writeback_faults_enabled() {
            return Validated::fail(Reason::HandAuthoredOnly);
        }
        let docs = candidates(state);
        check(
            state.action.app_started && !docs.is_empty(),
            Reason::NoDocumentsAvailable,
        )
        .map(|_| {
            (
                2,
                prop::sample::select(docs)
                    .prop_map(|doc| ArmWriteFailure { doc })
                    .boxed(),
            )
        })
    }
}

impl TransitionRef<ReferenceState> for ArmWriteFailure {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            state.action.app_started && candidates(state).contains(&self.doc),
            Reason::NoDocumentsAvailable,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        // A refused write leaves the file behind its store, which is exactly
        // what `write_churn` means to the oracle — the end state is the one
        // without the fault, reached once the fault ends.
        state.hold_write_backs(&self.doc);
    }
}

crate::cap_transition! {
    ArmWriteFailure: SutWriteFaults,
    |me, _state, sut| {
        sut.arm_write_failure(&me.doc).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ArmWriteFailure {
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

//! Transition: the file watcher loses the next change of a document's file.
//!
//! @pbt rung external
//!   arms the in-memory vault to deliver that change under a path no vault
//!   format matches.
//! @pbt covers write-back-state-machine — a watcher event is only a hint; the
//!   file still converges without it
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

use super::arm_write_churn::writeback_faults_enabled;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the file watcher loses the next change of the file of document {doc}")]
pub struct DropNextWatcherEvent {
    pub doc: EntityUri,
}

impl TransitionFactory<ReferenceState> for DropNextWatcherEvent {
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
                    .prop_map(|doc| DropNextWatcherEvent { doc })
                    .boxed(),
            )
        })
    }
}

impl TransitionRef<ReferenceState> for DropNextWatcherEvent {
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
    DropNextWatcherEvent: SutWriteFaults,
    |me, _state, sut| {
        sut.drop_next_watcher_event(&me.doc).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for DropNextWatcherEvent {
    fn expected_sql<R: holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        _: &R,
    ) -> ExpectedSql {
        ExpectedSql {
            reads: 0,
            writes: 0,
            ddl: 0,
            tolerance: 10,
        }
    }
}

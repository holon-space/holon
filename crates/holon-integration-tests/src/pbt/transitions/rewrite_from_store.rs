//! Transition: the user rewrites an emptied file from the store.
//!
//! @pbt rung production
//!   dispatches `writeback.rewrite_from_store` through the production operation
//!   dispatcher, asserts it is accepted and awaits its outcome.
//! @pbt covers write-back-state-machine — the file gets the store's content
//!   back and the store keeps every block
//!
//! Generated only for a document whose emptied file Holon refused.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutWriteFaults;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::accept_removal::refused_docs;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the user rewrites the emptied file of document {doc} from the store")]
pub struct RewriteFromStore {
    pub doc: EntityUri,
}

impl TransitionFactory<ReferenceState> for RewriteFromStore {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let docs = refused_docs(state);
        check(
            state.action.app_started && !docs.is_empty(),
            Reason::NoDocumentsAvailable,
        )
        .map(|_| {
            (
                3,
                prop::sample::select(docs)
                    .prop_map(|doc| RewriteFromStore { doc })
                    .boxed(),
            )
        })
    }
}

impl TransitionRef<ReferenceState> for RewriteFromStore {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            state.action.app_started && refused_docs(state).contains(&self.doc),
            Reason::NoDocumentsAvailable,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state
            .files
            .refused
            .remove(&self.doc)
            .expect("precondition: the document is refused");
        let name = crate::pbt::copies_model::file_name_of(state, &self.doc);
        state
            .conditions
            .clear(&name, holon_api::ConditionKind::VAULT_FILE_EMPTIED);
        state
            .conditions
            .clear(&name, holon_api::ConditionKind::REWRITE_FROM_STORE_FAILED);
    }
}

crate::cap_transition! {
    RewriteFromStore: SutWriteFaults,
    |me, _state, sut| {
        sut.rewrite_from_store(&me.doc).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for RewriteFromStore {
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

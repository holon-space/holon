//! Transition: the user accepts that an emptied file removes its document's
//! blocks.
//!
//! @pbt rung production
//!   dispatches `writeback.accept_removal` through the production operation
//!   dispatcher, asserts it is accepted and awaits its outcome.
//! @pbt covers write-back-state-machine — an accepted removal deletes the
//!   blocks only when the store did not change after the refusal; otherwise
//!   the store content goes back to the file
//!
//! Generated only for a document whose emptied file Holon refused.

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefApplyMutationMut;
use holon_pbt_core::capabilities::SutWriteFaults;
use holon_pbt_core::types::Mutation;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("the user accepts that the emptied file of document {doc} removes its blocks")]
pub struct AcceptRemoval {
    pub doc: EntityUri,
}

pub(super) fn refused_docs(state: &ReferenceState) -> Vec<EntityUri> {
    state.files.refused_docs().cloned().collect()
}

impl TransitionFactory<ReferenceState> for AcceptRemoval {
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
                    .prop_map(|doc| AcceptRemoval { doc })
                    .boxed(),
            )
        })
    }
}

impl TransitionRef<ReferenceState> for AcceptRemoval {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            state.action.app_started && refused_docs(state).contains(&self.doc),
            Reason::NoDocumentsAvailable,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let at_refusal = state
            .files
            .refused
            .remove(&self.doc)
            .expect("precondition: the document is refused");
        let name = crate::pbt::copies_model::file_name_of(state, &self.doc);
        state
            .conditions
            .clear(&name, holon_api::ConditionKind::VAULT_FILE_EMPTIED);
        if at_refusal != state.doc_rows(&self.doc) {
            state
                .conditions
                .raise(name, holon_api::ConditionKind::REMOVAL_NOT_ACCEPTED);
            return;
        }
        state
            .conditions
            .clear(&name, holon_api::ConditionKind::REMOVAL_NOT_ACCEPTED);
        let children: Vec<EntityUri> = state
            .domain
            .block_state
            .blocks
            .values()
            .filter(|b| b.parent_id == self.doc)
            .map(|b| b.id.clone())
            .collect();
        for id in children {
            state.apply_content_mutation(&Mutation::Delete { id: id.clone() }, true);
            state.clear_focus_if_deleted(&id);
        }
    }
}

crate::cap_transition! {
    AcceptRemoval: SutWriteFaults,
    |me, _state, sut| {
        sut.accept_removal(&me.doc).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for AcceptRemoval {
    fn expected_sql<R: holon_pbt_core::capabilities::RefSqlCardinality>(
        &self,
        state: &R,
    ) -> ExpectedSql {
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(state.document_count()),
            writes: 0,
            ddl: 0,
            tolerance: 200,
        }
    }
}

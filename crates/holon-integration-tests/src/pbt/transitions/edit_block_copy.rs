//! Transition: the external editor changes the task keyword of a pasted copy
//! in the file that holds it, while the owner's file still holds the block.
//!
//! @pbt rung external
//!   rewrites the copy's headline in its page file through the in-memory
//!   vault and lets the production FileSyncController ingest it.
//! @pbt covers move-block-between-files — a copy edited on disk while Holon
//!   edits the same block: the later release merges both, or refuses and
//!   discloses when both changed the keyword apart (D229.b)

use holon_api::EntityUri;
use holon_orgmode::OrgDocumentExt;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::SutSeamMutate;
use holon_pbt_core::types::CycleTarget;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::cdc_tolerance;

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template(
    "an external editor sets the copy of block {block_id} in document {copy_doc} to state \
     {state} and saves it"
)]
pub struct EditBlockCopy {
    pub block_id: EntityUri,
    pub copy_doc: EntityUri,
    pub state: CycleTarget,
}

/// The copies whose file reads the production task keywords, with each
/// keyword the copy does not carry yet.
fn candidates(state: &ReferenceState) -> Vec<EditBlockCopy> {
    let mut out = Vec::new();
    for (id, copy) in &state.files.copies {
        for (doc, on_disk) in &copy.files {
            let default_keywords = state
                .domain
                .block_state
                .blocks
                .get(doc)
                .is_some_and(|page| page.todo_keywords().is_none());
            if !default_keywords {
                continue;
            }
            let current = on_disk.disk_task_state.clone().unwrap_or_default();
            for target in CycleTarget::ALL {
                if target.keyword() != current {
                    out.push(EditBlockCopy {
                        block_id: id.clone(),
                        copy_doc: doc.clone(),
                        state: target,
                    });
                }
            }
        }
    }
    out
}

impl TransitionFactory<ReferenceState> for EditBlockCopy {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = candidates(state);
        if candidates.is_empty() {
            return Validated::fail(Reason::NoCopyStands);
        }
        let edited_in_holon = state.copies_edited_in_holon();
        let ripe: Vec<EditBlockCopy> = candidates
            .iter()
            .filter(|c| {
                edited_in_holon.contains(&c.block_id)
                    && c.state.keyword() != state.task_state_of(&c.block_id).unwrap_or_default()
            })
            .cloned()
            .collect();
        if !ripe.is_empty() {
            return Validated::Good((150, prop::sample::select(ripe).boxed()));
        }
        Validated::Good((60, prop::sample::select(candidates).boxed()))
    }
}

impl TransitionRef<ReferenceState> for EditBlockCopy {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            candidates(state).iter().any(|c| {
                c.block_id == self.block_id && c.copy_doc == self.copy_doc && c.state == self.state
            }),
            Reason::NoCopyStands,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let keyword = self.state.keyword();
        state
            .files
            .copies
            .get_mut(&self.block_id)
            .expect("precondition: the copy stands")
            .files
            .get_mut(&self.copy_doc)
            .expect("precondition: this file holds the copy")
            .disk_task_state = (!keyword.is_empty()).then(|| keyword.to_string());
    }
}

crate::cap_transition! {
    EditBlockCopy: SutSeamMutate,
    where R: [ RefLifecycle + RefBlockTree + RefDocuments ],
    |me, _state, sut| {
        sut.edit_block_copy(&me.block_id, &me.copy_doc, me.state).await;
    }
    sql_budget: |_me, state| {
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(docs) + 40,
            writes: 4,
            ddl: 0,
            tolerance: cdc_tolerance(blocks, docs) + 16,
        }
    }
}

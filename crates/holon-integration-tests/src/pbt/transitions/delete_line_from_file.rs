//! Transition: the external editor deletes the line of a copied heading's
//! child from one file while a copy of the heading stands.
//!
//! @pbt rung external
//!   cuts the child's section out of the page file through the in-memory
//!   vault and lets the production FileSyncController ingest it.
//! @pbt covers move-block-between-files — a child deleted from its own file
//!   is put back while a copy holds it, and the put-back is disclosed; the
//!   same deletion in the copy lets it stand (D229.b)

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutSeamMutate;
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
    "an external editor deletes block {block_id}, under the copied block {root}, from the file \
     of document {doc} and saves it"
)]
pub struct DeleteLineFromFile {
    pub block_id: EntityUri,
    pub root: EntityUri,
    pub doc: EntityUri,
}

/// The owner's file loses a childless member of a copied heading: Holon puts
/// it back while a copy holds it. A copy loses a member whose deletion Holon
/// undid: once no copy holds it, the deletion stands.
fn candidates(state: &ReferenceState) -> Vec<DeleteLineFromFile> {
    let mut out = Vec::new();
    for (root, copy) in &state.files.copies {
        if copy.conflict {
            continue;
        }
        let owner = state
            .owner_doc_of(root)
            .unwrap_or_else(|| panic!("copied block {root} is in no file"));
        let childless_in_store = |id: &EntityUri| {
            !state
                .domain
                .block_state
                .blocks
                .values()
                .any(|b| b.parent_id == *id)
        };
        for member in state.domain.block_state.subtree_ids(root).iter().skip(1) {
            // A copied heading leaves its own file through `FinishCutPaste`.
            if copy.undone.contains_key(member)
                || !childless_in_store(member)
                || state.files.copies.contains_key(member)
            {
                continue;
            }
            out.push(DeleteLineFromFile {
                block_id: member.clone(),
                root: root.clone(),
                doc: owner.clone(),
            });
        }
        for (member, undone) in &copy.undone {
            for doc in &undone.holders {
                let blocks = &copy.files[doc].blocks;
                if blocks.iter().any(|b| b.parent_id == *member) {
                    continue;
                }
                out.push(DeleteLineFromFile {
                    block_id: member.clone(),
                    root: root.clone(),
                    doc: doc.clone(),
                });
            }
        }
    }
    out
}

impl TransitionFactory<ReferenceState> for DeleteLineFromFile {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = candidates(state);
        if candidates.is_empty() {
            return Validated::fail(Reason::NoCopyStands);
        }
        let weight = if candidates
            .iter()
            .any(|c| state.files.copies[&c.root].files.contains_key(&c.doc))
        {
            800
        } else {
            600
        };
        Validated::Good((weight, prop::sample::select(candidates).boxed()))
    }
}

impl TransitionRef<ReferenceState> for DeleteLineFromFile {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            candidates(state)
                .iter()
                .any(|c| c.block_id == self.block_id && c.root == self.root && c.doc == self.doc),
            Reason::NoCopyStands,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let copy = state
            .files
            .copies
            .get_mut(&self.root)
            .expect("precondition: the copy stands");
        if let Some(on_disk) = copy.files.get_mut(&self.doc) {
            on_disk.blocks.retain(|b| b.id != self.block_id);
            copy.undone
                .get_mut(&self.block_id)
                .expect("precondition: the deletion was undone")
                .holders
                .remove(&self.doc);
            state.settle_undone(&self.root);
            return;
        }
        let holders: std::collections::BTreeSet<EntityUri> = copy
            .files
            .iter()
            .filter(|(_, on_disk)| on_disk.blocks.iter().any(|b| b.id == self.block_id))
            .map(|(doc, _)| doc.clone())
            .collect();
        let put_back = state.domain.block_state.blocks[&self.block_id].clone();
        state
            .files
            .copies
            .get_mut(&self.root)
            .expect("precondition: the copy stands")
            .undone
            .insert(
                self.block_id.clone(),
                crate::pbt::file_adapter_state::UndoneMember { holders, put_back },
            );
        state.settle_undone(&self.root);
    }
}

crate::cap_transition! {
    DeleteLineFromFile: SutSeamMutate,
    where R: [ RefLifecycle + RefBlockTree + RefDocuments ],
    |me, _state, sut| {
        sut.delete_line_from_file(&me.block_id, &me.root, &me.doc).await;
    }
    sql_budget: |_me, state| {
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: holon_pbt_core::budget::cdc_drain_floor(docs) + 60,
            writes: 6,
            ddl: 0,
            tolerance: cdc_tolerance(blocks, docs) + 24,
        }
    }
}

//! Transition: the first save of an external cut & paste — the target file
//! with the pasted copy (at its top) is saved and ingested while the source
//! file on disk still holds the block.
//!
//! @pbt rung external
//!   rewrites the target page file through the in-memory vault and lets the
//!   production FileSyncController ingest it.
//! @pbt covers move-block-between-files — the invariants run while both files
//!   hold the block: both copies stay on disk, Holon's edits reach both files,
//!   and a condition names the block, its owner's file and the file with the
//!   copy (D229.b)

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefTaskStateToggle;
use holon_pbt_core::capabilities::SutSeamMutate;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use super::move_block_between_files::another_copy_candidates;
use super::move_block_between_files::candidates;
use super::move_block_between_files::preconditions_of_a_cut;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::cdc_tolerance;

/// The target of a cut & paste is saved and ingested while the source on disk
/// still holds the block.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template(
    "an external editor pastes block {block_id} from document {source_doc} into document \
     {target_doc} and saves only the document it pasted into"
)]
pub struct PasteBlockCopy {
    pub block_id: EntityUri,
    pub source_doc: EntityUri,
    pub target_doc: EntityUri,
}

impl TransitionFactory<ReferenceState> for PasteBlockCopy {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    /// The copy rules write the owner's and the copy's files back; without
    /// the org write-back leg the owner's disk is not the store's.
    fn required_wiring() -> holon_pbt_core::RequiredWiring {
        holon_pbt_core::RequiredWiring::HasStorage(holon_pbt_core::StorageAdapter::Org)
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let mut candidates = candidates(state);
        candidates.extend(another_copy_candidates(state));
        vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!candidates.is_empty(), Reason::PreconditionFailed),
        ]
        .into_iter()
        .collect::<Validated<Vec<()>, _>>()
        .map(move |_| {
            // A block the main panel shows with a task toggle is pasted more
            // often, so Holon can toggle it while the copy stands.
            let toggles = state.rendered_state_toggle_ids();
            let shown: Vec<(EntityUri, EntityUri, EntityUri)> = candidates
                .iter()
                .filter(|(id, _, _)| toggles.contains(id))
                .cloned()
                .collect();
            // A heading with children too, so the editor can delete a child
            // line while the copy stands.
            let blocks = &state.domain.block_state.blocks;
            let parents: Vec<(EntityUri, EntityUri, EntityUri)> = candidates
                .iter()
                .filter(|(id, _, _)| blocks.values().any(|b| b.parent_id == *id))
                .cloned()
                .collect();
            let mut picks = vec![(1, prop::sample::select(candidates).boxed())];
            for (weight, biased) in [(4, shown), (10, parents)] {
                if !biased.is_empty() {
                    picks.push((weight, prop::sample::select(biased).boxed()));
                }
            }
            let pick = proptest::strategy::Union::new_weighted(picks);
            let strat = pick
                .prop_map(|(block_id, source_doc, target_doc)| PasteBlockCopy {
                    block_id,
                    source_doc,
                    target_doc,
                })
                .boxed();
            (100, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for PasteBlockCopy {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        if state.files.copies.contains_key(&self.block_id) {
            return check(
                another_copy_candidates(state).contains(&(
                    self.block_id.clone(),
                    self.source_doc.clone(),
                    self.target_doc.clone(),
                )),
                Reason::PreconditionFailed,
            );
        }
        preconditions_of_a_cut(state, &self.block_id, &self.source_doc, &self.target_doc)
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        state.paste_copy(&self.block_id, &self.target_doc);
    }
}

crate::cap_transition! {
    PasteBlockCopy: SutSeamMutate,
    where R: [ RefLifecycle + RefBlockTree + RefDocuments ],
    |me, _state, sut| {
        sut.paste_block_copy(&me.block_id, &me.source_doc, &me.target_doc).await;
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

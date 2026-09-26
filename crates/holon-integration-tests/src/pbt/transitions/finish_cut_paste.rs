//! Transition: the second save of an external cut & paste whose pasted copy
//! is on disk in both files — the owner's file is saved without the block,
//! or the copy is deleted from the file that holds it.
//!
//! @pbt rung external
//!   rewrites one page file through the in-memory vault and lets the
//!   production FileSyncController ingest it.
//! @pbt covers move-block-between-files — the later save resolves a block
//!   held by two files: adopted and merged with Holon's edits, refused when
//!   both were edited apart (the next deletion decides), or left with its
//!   owner (D229.b)

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutSeamMutate;
use holon_pbt_core::types::CutPasteFinish;
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

/// The second save of a cut & paste whose copy is on disk in both files.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template(
    "the cut of block {block_id} from document {source_doc} to document {target_doc} ends: \
     {finish}"
)]
pub struct FinishCutPaste {
    pub block_id: EntityUri,
    /// The document whose file owns the block now.
    pub source_doc: EntityUri,
    /// The document whose file holds the copy.
    pub target_doc: EntityUri,
    pub finish: CutPasteFinish,
}

/// The app still holds `id`'s pasted ancestor, which a release merges
/// against; the model describes no release without it.
fn base_known(state: &ReferenceState, id: &EntityUri) -> bool {
    state.files.copies[id].base_known
}

/// Every standing copy, as `(block, owner document, copy document)`.
fn standing(state: &ReferenceState) -> Vec<(EntityUri, EntityUri, EntityUri)> {
    state
        .files
        .copies
        .iter()
        .flat_map(|(id, copy)| {
            let owner = state
                .owner_doc_of(id)
                .unwrap_or_else(|| panic!("copied block {id} is in no file"));
            copy.files
                .keys()
                .map(move |doc| (id.clone(), owner.clone(), doc.clone()))
        })
        .collect()
}

impl TransitionFactory<ReferenceState> for FinishCutPaste {
    fn required_caps() -> Vec<holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;

    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let standing = standing(state);
        if standing.is_empty() {
            return Validated::fail(Reason::NoCopyStands);
        }
        let edited_apart = state.copies_edited_apart();
        let ripe: Vec<_> = standing
            .iter()
            .filter(|(id, _, _)| edited_apart.contains(id) && base_known(state, id))
            .cloned()
            .collect();
        if !ripe.is_empty() {
            let strat = prop::sample::select(ripe)
                .prop_map(|(block_id, source_doc, target_doc)| FinishCutPaste {
                    block_id,
                    source_doc,
                    target_doc,
                    finish: CutPasteFinish::SourceSaved,
                })
                .boxed();
            return Validated::Good((300, strat));
        }
        // The owner's save, which adopts the copy, is the feature's main path.
        let finish = prop_oneof![
            3 => Just(CutPasteFinish::SourceSaved),
            1 => Just(CutPasteFinish::CopyDeletedFromTarget),
        ];
        let known: Vec<bool> = standing
            .iter()
            .map(|(id, _, _)| base_known(state, id))
            .collect();
        let strat = (
            prop::sample::select(standing.into_iter().zip(known).collect::<Vec<_>>()),
            finish,
        )
            .prop_map(
                |(((block_id, source_doc, target_doc), known), finish)| FinishCutPaste {
                    block_id,
                    source_doc,
                    target_doc,
                    finish: if known {
                        finish
                    } else {
                        CutPasteFinish::CopyDeletedFromTarget
                    },
                },
            )
            .boxed();
        Validated::Good((12, strat))
    }
}

impl TransitionRef<ReferenceState> for FinishCutPaste {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        check(
            standing(state).contains(&(
                self.block_id.clone(),
                self.source_doc.clone(),
                self.target_doc.clone(),
            )) && (self.finish == CutPasteFinish::CopyDeletedFromTarget
                || base_known(state, &self.block_id)),
            Reason::NoCopyStands,
        )
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        match self.finish {
            CutPasteFinish::SourceSaved => state.release_copy(&self.block_id),
            CutPasteFinish::CopyDeletedFromTarget => {
                state.drop_copy_file(&self.block_id, &self.target_doc);
            }
        }
    }
}

crate::cap_transition! {
    FinishCutPaste: SutSeamMutate,
    where R: [ RefLifecycle + RefBlockTree + RefDocuments ],
    |me, _state, sut| {
        sut.finish_cut_paste(&me.block_id, &me.source_doc, &me.target_doc, me.finish).await;
    }
    sql_budget: |_me, state| {
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: 2 * (holon_pbt_core::budget::cdc_drain_floor(docs) + 22) + 30,
            writes: 8,
            ddl: 0,
            tolerance: cdc_tolerance(blocks, docs) + 32,
        }
    }
}

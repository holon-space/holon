//! Transition: bulk external add of blocks via org file write (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:833-860` (generator),
//! `state_machine.rs:3187-3189` (precondition),
//! `state_machine.rs:2457-2480` (ref-state apply),
//! `sut.rs:1565-1847` (SUT apply), and
//! `transition_budgets.rs:233-251` (expected SQL).

use holon_pbt_core::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use holon_pbt_core::capabilities::{
    RefDocuments, RefLayoutInteract, RefLayoutMutate, RefLifecycle, SutSeamMutate,
};
use holon_pbt_core::{TransitionFactory, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    CACHE_EVENT_READS, ExpectedSql, REACTIVE_BASE, READS_PER_WATCH, cdc_tolerance,
};

use holon_api::EntityUri;
use holon_api::block::Block;

/// Add multiple blocks to a document by writing an updated org file.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BulkExternalAdd {
    pub doc_uri: EntityUri,
    #[serde(with = "holon_api::block::block_wire_vec")]
    pub blocks: Vec<Block>,
}

impl<R: RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate> TransitionFactory<R>
    for BulkExternalAdd
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let doc_uris: Vec<EntityUri> = state.document_uris();
        check(!doc_uris.is_empty(), Reason::NoDocumentsAvailable).map(|_| {
            // Boost weight when most documents have no editable Text content.
            // Without seeded content, every edit-path generator that filters on
            // `main_editable_descendants` (SplitBlock, Indent, EditViaViewModel,
            // ClickBlock-as-Main, …) returns `None` and the PBT never reaches
            // editing code. Once docs hold Text blocks the weight drops back to
            // base so the rest of the strategy can run.
            let is_empty_doc = |doc_uri: &EntityUri| -> bool { !state.doc_has_editable_text(doc_uri) };
            let empty_doc_uris: Vec<EntityUri> = doc_uris
                .iter()
                .filter(|u| is_empty_doc(u))
                .cloned()
                .collect();
            let total_docs = doc_uris.len();
            let weight: u32 = if empty_doc_uris.len() * 2 >= total_docs {
                100
            } else {
                1
            };

            // Prefer empty docs when any exist: the multi-block-into-empty-parent
            // path is the cleanest exerciser of the Full-mode CDC ordering invariant
            // (org-parser-assigned sort_keys must agree with the Loro tree's
            // fractional indices). Sampling from the empty subset deterministically
            // hits that path on every BulkExternalAdd, instead of leaving it to a
            // random `select(doc_uris)` to land there.
            let candidate_docs = if !empty_doc_uris.is_empty() {
                empty_doc_uris
            } else {
                doc_uris
            };

            let next_id = state.next_block_id();
            let strat = (
                prop::sample::select(candidate_docs),
                prop::collection::vec(
                    (
                        crate::pbt::generators::bulk_content_strategy(),
                        // Parent selector (extended-gen axis 2): block `i`
                        // parents to `sel % (i + 1)` — 0 = the doc, k = bulk
                        // block k-1. Well-founded (earlier blocks only).
                        proptest::num::u8::ANY,
                    ),
                    3..=10,
                ),
            )
                .prop_map(move |(doc_entity_uri, contents)| {
                    let blocks: Vec<Block> = contents
                        .into_iter()
                        .enumerate()
                        .map(|(i, (content, parent_sel))| {
                            let parent = match parent_sel as usize % (i + 1) {
                                k if k > 0 => {
                                    EntityUri::block(&format!("bulk-{}-{}", next_id, k - 1))
                                }
                                _ => doc_entity_uri.clone(),
                            };
                            Block::new_text(
                                EntityUri::block(&format!("bulk-{}-{}", next_id, i)),
                                parent,
                                content,
                            )
                        })
                        .collect();
                    BulkExternalAdd {
                        doc_uri: doc_entity_uri,
                        blocks,
                    }
                })
                .boxed();
            (weight, strat)
        })
    }
}

impl<R: RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate> TransitionRef<R>
    for BulkExternalAdd
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.has_document_uri(&self.doc_uri),
                Reason::NoDocumentsAvailable,
            ),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole bulk-add reference effect (org round-trip normalization, doc
        // ownership, canonical re-sequencing, block-id counter advance) lives in
        // `RefLayoutMutate::bulk_add_blocks`.
        state.bulk_add_blocks(&self.doc_uri, &self.blocks);
    }
}

crate::cap_transition! {
    BulkExternalAdd: SutSeamMutate,
    where R: [ RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate ],
    |me, _state, sut| {
        sut.bulk_external_add(&me.doc_uri, &me.blocks).await;
    }
    sql_budget: |me, state| {
        let n = me.blocks.len();
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 1 + n + watches * READS_PER_WATCH,
            writes: n + 2,
            ddl: watches,
            // Each new block triggers org sync CDC; reactive base fires per CDC cycle.
            tolerance: cdc_tolerance(blocks + n, docs) + n * 3,
        }
    }
}

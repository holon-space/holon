//! Transition: bulk external add of blocks via org file write (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:833-860` (generator),
//! `state_machine.rs:3187-3189` (precondition),
//! `state_machine.rs:2457-2480` (ref-state apply),
//! `sut.rs:1565-1847` (SUT apply), and
//! `transition_budgets.rs:233-251` (expected SQL).

use crate::pbt::validation::{Reason, check};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::local_caps::SutSeamMutate;
use crate::pbt::reference_state::ReferenceState;
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    CACHE_EVENT_READS, ExpectedSql, REACTIVE_BASE, READS_PER_WATCH, cdc_tolerance,
};

use holon_api::ContentType;
use holon_api::EntityUri;
use holon_api::block::Block;

use crate::assign_reference_sequences_canonical;
use crate::pbt::types::{apply_org_headline_tag_split, normalize_content_for_org_roundtrip};

/// Add multiple blocks to a document by writing an updated org file.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct BulkExternalAdd {
    pub doc_uri: EntityUri,
    #[serde(with = "holon_api::block::block_wire_vec")]
    pub blocks: Vec<Block>,
}

impl TransitionFactory<ReferenceState> for BulkExternalAdd {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn crate::pbt::local_caps::SutSeamMutate,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let doc_uris: Vec<EntityUri> = state.files.documents.keys().cloned().collect();
        check(!doc_uris.is_empty(), Reason::NoDocumentsAvailable).map(|_| {
            // Boost weight when most documents have no editable Text content.
            // Without seeded content, every edit-path generator that filters on
            // `main_editable_descendants` (SplitBlock, Indent, EditViaViewModel,
            // ClickBlock-as-Main, …) returns `None` and the PBT never reaches
            // editing code. Once docs hold Text blocks the weight drops back to
            // base so the rest of the strategy can run.
            let is_empty_doc = |doc_uri: &EntityUri| -> bool {
                !state.domain.block_state.blocks.values().any(|b| {
                    b.parent_id == *doc_uri
                        && b.content_type == ContentType::Text
                        && !b.is_page()
                        && !state.domain.layout_blocks.contains(&b.id)
                })
            };
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

            let next_id = state.domain.block_state.next_id;
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

impl TransitionRef<ReferenceState> for BulkExternalAdd {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            check(
                state.files.documents.contains_key(&self.doc_uri),
                Reason::NoDocumentsAvailable,
            ),
        ];

        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        // Add all blocks to the reference state, normalizing each
        // block's content the same way `Mutation::apply_to` does for
        // Create. The org renderer round-trips through the parser
        // (which `.trim()`s headlines and `.trim_end()`s content),
        // so the ref must mirror that normalization or `text(col(...))`
        // displays diverge by the trailing-whitespace the parser
        // strips. Without this, `inv-displayed-text` panics on bulk
        // blocks whose generator-produced content ends in a space.
        for block in &self.blocks {
            let mut block = block.clone();
            block.content = normalize_content_for_org_roundtrip(&block.content, block.content_type);
            // A trailing `:tag:` group on the title line re-parses as org TAGS.
            apply_org_headline_tag_split(&mut block);
            let id = block.id.clone();
            state.domain.block_state.blocks.insert(id.clone(), block);
            // Register doc ownership so WriteOrgFile::apply_to_ref's delete
            // cascade can find these on a subsequent file rewrite.
            state
                .domain
                .block_state
                .block_documents
                .insert(id, self.doc_uri.clone());
        }
        // BulkExternalAdd serializes via serialize_blocks_to_org (canonical order)
        let mut all_blocks: Vec<Block> =
            state.domain.block_state.blocks.values().cloned().collect();
        assign_reference_sequences_canonical(&mut all_blocks);
        state.domain.block_state.blocks =
            all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        state.rebuild_profile_tracking();
        state.domain.block_state.next_id += self.blocks.len();
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutSeamMutate> TransitionImpl<ReferenceState, S> for BulkExternalAdd {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.bulk_external_add(&self.doc_uri, &self.blocks).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for BulkExternalAdd {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let n = self.blocks.len();
        let watches = state.mcp.active_watches.len();
        let blocks = state.domain.block_state.blocks.len();
        let docs = state.files.documents.len();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 1 + n + watches * READS_PER_WATCH,
            writes: n + 2,
            ddl: watches,
            // Each new block triggers org sync CDC; reactive base fires per CDC cycle.
            tolerance: cdc_tolerance(blocks + n, docs) + n * 3,
        }
    }
}

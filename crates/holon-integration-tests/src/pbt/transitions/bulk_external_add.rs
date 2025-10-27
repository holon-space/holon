//! Transition: bulk external add of blocks via org file write (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:833-860` (generator),
//! `state_machine.rs:3187-3189` (precondition),
//! `state_machine.rs:2457-2480` (ref-state apply),
//! `sut.rs:1565-1847` (SUT apply), and
//! `transition_budgets.rs:233-251` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;

use super::E2ETransitionImpl;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::{E2ETransitionFactory, SutHandle};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    CACHE_EVENT_READS, ExpectedSql, REACTIVE_BASE, READS_PER_WATCH, cdc_tolerance,
};

use holon_api::EntityUri;
use holon_api::block::Block;

use crate::assign_reference_sequences_canonical;
use crate::pbt::types::normalize_content_for_org_roundtrip;

/// Add multiple blocks to a document by writing an updated org file.
#[derive(Clone, Debug)]
pub struct BulkExternalAdd {
    pub doc_uri: EntityUri,
    pub blocks: Vec<Block>,
}

impl E2ETransitionFactory for BulkExternalAdd {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        let doc_uris: Vec<EntityUri> = state.documents.keys().cloned().collect();
        if doc_uris.is_empty() {
            return None;
        }
        let next_id = state.block_state.next_id;
        let strat = (
            prop::sample::select(doc_uris),
            prop::collection::vec("[a-zA-Z][a-zA-Z0-9 ]{0,20}", 3..=10),
        )
            .prop_map(move |(doc_entity_uri, contents)| {
                let blocks: Vec<Block> = contents
                    .into_iter()
                    .enumerate()
                    .map(|(i, content)| {
                        Block::new_text(
                            EntityUri::block(&format!("bulk-{}-{}", next_id, i)),
                            doc_entity_uri.clone(),
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
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for BulkExternalAdd {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started && state.documents.contains_key(&self.doc_uri)
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
            state.block_state.blocks.insert(block.id.clone(), block);
        }
        // BulkExternalAdd serializes via serialize_blocks_to_org (canonical order)
        let mut all_blocks: Vec<Block> = state.block_state.blocks.values().cloned().collect();
        assign_reference_sequences_canonical(&mut all_blocks);
        state.block_state.blocks = all_blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        state.rebuild_profile_tracking();
        state.block_state.next_id += self.blocks.len();
    }

    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_bulk_external_add(&self.doc_uri, &self.blocks, ref_state)
            .await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let n = self.blocks.len();
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 1 + n + watches * READS_PER_WATCH,
            writes: n + 2,
            ddl: watches,
            // Each new block triggers org sync CDC; reactive base fires per CDC cycle.
            tolerance: cdc_tolerance(blocks + n, docs) + n * 3,
        }
    }
}

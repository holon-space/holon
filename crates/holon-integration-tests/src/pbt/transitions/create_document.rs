//! Transition: create a new document (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:447-455` (generator),
//! `state_machine.rs:3117` (precondition),
//! `state_machine.rs:2126-2147` (ref-state apply),
//! `sut.rs:1275-1295` (SUT apply), and
//! `transition_budgets.rs:218-228` (expected SQL).

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

/// Create a new empty document (post-startup).
#[derive(Clone, Debug)]
pub struct CreateDocument {
    pub file_name: String,
}

impl E2ETransitionFactory for CreateDocument {
    fn weighted_generator(state: &ReferenceState) -> Option<(u32, BoxedStrategy<Self>)> {
        if !state.app_started {
            return None;
        }
        let next_doc_id = state.next_doc_id;
        let strat = Just(CreateDocument {
            file_name: format!("doc_{}.org", next_doc_id),
        })
        .boxed();
        Some((1, strat))
    }
}

#[allow(async_fn_in_trait)]
impl E2ETransitionImpl for CreateDocument {
    fn preconditions(&self, state: &ReferenceState) -> bool {
        state.app_started
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let doc_uri = state.next_synthetic_doc_uri();
        state
            .documents
            .insert(doc_uri.clone(), self.file_name.clone());

        let doc_name = std::path::Path::new(self.file_name.as_str())
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or(&self.file_name)
            .to_string();
        let mut doc_block = Block::new_text(doc_uri.clone(), EntityUri::no_parent(), doc_name);
        doc_block.set_page(true);
        // New empty documents don't have #+TODO: headers — keywords only
        // appear after the file is written with content. The on_file_changed
        // handler syncs parsed keywords to the document block.
        state.block_state.blocks.insert(doc_uri.clone(), doc_block);
        state
            .block_state
            .block_documents
            .insert(doc_uri.clone(), doc_uri);
    }

    async fn apply_to_sut(&self, ref_state: &ReferenceState, sut: &mut dyn SutHandle) {
        sut.apply_create_document(&self.file_name, ref_state).await;
    }

    #[cfg(feature = "otel-testing")]
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.active_watches.len();
        let blocks = state.block_state.blocks.len();
        let docs = state.documents.len();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 4 + watches * READS_PER_WATCH,
            writes: 4,
            ddl: 0,
            // Use blocks + 5 since CreateDocument itself adds blocks to the DB.
            tolerance: cdc_tolerance(blocks + 5, docs + 1) + watches * 4,
        }
    }
}

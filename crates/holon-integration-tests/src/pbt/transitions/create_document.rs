//! Transition: create a new document (post-startup).
//!
//! Mirrors the legacy logic split across `state_machine.rs:447-455` (generator),
//! `state_machine.rs:3117` (precondition),
//! `state_machine.rs:2126-2147` (ref-state apply),
//! `sut.rs:1275-1295` (SUT apply), and
//! `transition_budgets.rs:218-228` (expected SQL).

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::local_caps::SutAppLifecycle;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    CACHE_EVENT_READS, ExpectedSql, REACTIVE_BASE, READS_PER_WATCH, cdc_tolerance,
};

use holon_api::EntityUri;
use holon_api::block::Block;

/// Create a new empty document (post-startup).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateDocument {
    pub file_name: String,
}

impl TransitionFactory<ReferenceState> for CreateDocument {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn crate::pbt::local_caps::SutAppLifecycle,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let instance = CreateDocument {
            file_name: format!("doc_{}.org", state.action.next_doc_id),
        };
        instance.preconditions(state).map(|_| {
            let strat = Just(instance).boxed();
            (1, strat)
        })
    }
}

impl TransitionRef<ReferenceState> for CreateDocument {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.action.app_started, Reason::AppNotStarted)];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let doc_uri = state.next_synthetic_doc_uri();
        state
            .files
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
        state
            .domain
            .block_state
            .blocks
            .insert(doc_uri.clone(), doc_block);
        state
            .domain
            .block_state
            .block_documents
            .insert(doc_uri.clone(), doc_uri);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutAppLifecycle> TransitionImpl<ReferenceState, S> for CreateDocument {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.create_document(&self.file_name).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for CreateDocument {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.mcp.active_watches.len();
        let blocks = state.domain.block_state.blocks.len();
        let docs = state.files.documents.len();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 4 + watches * READS_PER_WATCH,
            writes: 4,
            ddl: 0,
            // Use blocks + 5 since CreateDocument itself adds blocks to the DB.
            tolerance: cdc_tolerance(blocks + 5, docs + 1) + watches * 4,
        }
    }
}

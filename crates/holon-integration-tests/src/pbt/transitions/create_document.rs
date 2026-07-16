//! Transition: create a new document (post-startup).
//!
//! @pbt rung external
//!   writes an empty org file into the watched org_root; the production
//!   FileSyncController ingests it and mints the page block.
//! @pbt covers doc-create-ingest — new-doc file -> watcher -> block_raw page
//!
//! Mirrors the legacy logic split across `state_machine.rs:447-455`
//! (generator), `state_machine.rs:3117` (precondition),
//! `state_machine.rs:2126-2147` (ref-state apply),
//! `sut.rs:1275-1295` (SUT apply), and
//! `transition_budgets.rs:218-228` (expected SQL).

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefDocumentsMut;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutAppLifecycle;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::CACHE_EVENT_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::READS_PER_WATCH;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::cdc_tolerance;

/// Create a new empty document (post-startup).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct CreateDocument {
    pub file_name: String,
}

impl<R: RefLifecycle + RefDocumentsMut> TransitionFactory<R> for CreateDocument {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let instance = CreateDocument {
            file_name: format!("doc_{}.org", state.next_doc_id()),
        };
        instance.preconditions(state).map(|_| {
            let strat = Just(instance).boxed();
            // Weight 2: feeds DeleteDocument's candidate pool; at weight 1 a
            // 16-case run created only 6 docs and never drew a deletion.
            (2, strat)
        })
    }
}

impl<R: RefLifecycle + RefDocumentsMut> TransitionRef<R> for CreateDocument {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> =
            vec![check(state.app_started(), Reason::AppNotStarted)];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole create-document reference effect (mint synthetic uri, register
        // filename, insert the empty page block) lives in
        // `RefDocumentsMut::insert_document`.
        state.insert_document(&self.file_name);
    }
}

crate::cap_transition! {
    CreateDocument: SutAppLifecycle,
    where R: [ RefLifecycle + RefDocumentsMut ],
    |me, _state, sut| {
        sut.create_document(&me.file_name).await;
    }
    sql_budget: |_me, state| {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 4 + watches * READS_PER_WATCH,
            writes: 4,
            ddl: 0,
            // Use blocks + 5 since CreateDocument itself adds blocks to the DB.
            tolerance: cdc_tolerance(blocks + 5, docs + 1) + watches * 4,
        }
    }
}

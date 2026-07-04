//! Transition: delete a previously created document (post-startup).
//!
//! @pbt rung external
//! @pbt covers doc-delete — fs delete -> watcher removal -> block_raw page drop
//!
//! Inverse of [`CreateDocument`](super::create_document::CreateDocument).
//! Models an EXTERNAL deletion — the user removing the org file outside
//! Holon (`rm` in the vault), the scenario the prod bug was observed in.
//! The SUT apply removes the file straight through the `FileSystem` port
//! (never an app command), so all the live system sees is a watcher event
//! for a path that no longer exists — exactly what a real out-of-app `rm`
//! produces. The reference model cascade-deletes the page block + its
//! descendants; SUT blocks that linger after the file vanished are the
//! divergence this transition exists to surface.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefDocuments;
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

/// Delete a document previously created by `CreateDocument` (post-startup).
///
/// Candidates are DELIBERATELY narrowed to the synthetic `doc_<n>.org` names
/// `CreateDocument` mints — seed pages and pre-startup-ingested user org files
/// are excluded so the deletion universe is exactly the create-inverse. This
/// narrowing is liftable once file-deletion convergence is proven on the
/// synthetic docs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I delete document {file_name}")]
pub struct DeleteDocument {
    pub file_name: String,
}

/// Whether `name` matches the synthetic `doc_<n>.org` pattern `CreateDocument`
/// generates (`format!("doc_{}.org", next_doc_id)`).
fn is_synthetic_doc_name(name: &str) -> bool {
    name.strip_prefix("doc_")
        .and_then(|rest| rest.strip_suffix(".org"))
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

fn deletable_doc_names<R: RefDocuments>(state: &R) -> Vec<String> {
    let mut names: Vec<String> = state
        .document_names()
        .into_iter()
        .filter(|name| is_synthetic_doc_name(name))
        .collect();
    names.sort();
    names
}

impl<R: RefLifecycle + RefDocumentsMut> TransitionFactory<R> for DeleteDocument {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = deletable_doc_names(state);
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(!candidates.is_empty(), Reason::NoDocumentsAvailable),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| {
                let strat = proptest::sample::select(candidates)
                    .prop_map(|file_name| DeleteDocument { file_name })
                    .boxed();
                // Weight 3: only enabled once a synthetic doc exists, and a
                // default-weight 16-case run drew it 0 times — deletion
                // convergence must not depend on HOLON_PBT_WEIGHTS overrides.
                (3, strat)
            })
    }
}

impl<R: RefLifecycle + RefDocumentsMut> TransitionRef<R> for DeleteDocument {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            // Shrinking re-checks this: the doc must still exist (an earlier
            // shrunk-away CreateDocument invalidates dependent deletes).
            check(
                state.has_document(&self.file_name),
                Reason::NoDocumentsAvailable,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole external-deletion reference effect (resolve uri, cascade-delete
        // page + descendants, re-canonicalize, clear dangling focus) lives in
        // `RefDocumentsMut::remove_document`.
        state.remove_document(&self.file_name);
    }
}

crate::cap_transition! {
    DeleteDocument: SutAppLifecycle,
    where R: [ RefLifecycle + RefDocumentsMut ],
    |me, _state, sut| {
        sut.delete_document(&me.file_name).await;
    }
    sql_budget: |_me, state| {
        let watches = state.active_watch_count();
        let blocks = state.block_count();
        let docs = state.document_count();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + 4 + watches * READS_PER_WATCH,
            writes: 4,
            ddl: 0,
            // Writes scale with the deleted subtree (cascade DELETEs + CDC),
            // so pad CreateDocument's tolerance shape with `blocks * 6`.
            tolerance: cdc_tolerance(blocks + 5, docs + 1) + watches * 4 + blocks * 6,
        }
    }
}

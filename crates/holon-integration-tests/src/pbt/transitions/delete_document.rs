//! Transition: delete a previously created document (post-startup).
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

use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::assign_reference_sequences_canonical;
use crate::pbt::local_caps::SutAppLifecycle;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::types::Mutation;
use crate::pbt::validation::{Reason, check};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{
    CACHE_EVENT_READS, ExpectedSql, REACTIVE_BASE, READS_PER_WATCH, cdc_tolerance,
};

use holon_api::EntityUri;
use holon_api::block::Block;

/// Delete a document previously created by `CreateDocument` (post-startup).
///
/// Candidates are DELIBERATELY narrowed to the synthetic `doc_<n>.org` names
/// `CreateDocument` mints — seed pages and pre-startup-ingested user org files
/// are excluded so the deletion universe is exactly the create-inverse. This
/// narrowing is liftable once file-deletion convergence is proven on the
/// synthetic docs.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
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

fn deletable_doc_names(state: &ReferenceState) -> Vec<String> {
    let mut names: Vec<String> = state
        .files
        .documents
        .values()
        .filter(|name| is_synthetic_doc_name(name))
        .cloned()
        .collect();
    names.sort();
    names
}

impl TransitionFactory<ReferenceState> for DeleteDocument {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn crate::pbt::local_caps::SutAppLifecycle,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let candidates = deletable_doc_names(state);
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
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

impl TransitionRef<ReferenceState> for DeleteDocument {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.action.app_started, Reason::AppNotStarted),
            // Shrinking re-checks this: the doc must still exist (an earlier
            // shrunk-away CreateDocument invalidates dependent deletes).
            check(
                state
                    .files
                    .documents
                    .values()
                    .any(|name| *name == self.file_name),
                Reason::NoDocumentsAvailable,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        let doc_uri = state
            .files
            .documents
            .iter()
            .find(|(_, name)| **name == self.file_name)
            .map(|(uri, _)| uri.clone())
            .unwrap_or_else(|| {
                panic!(
                    "DeleteDocument::apply_to_ref: '{}' not in files.documents (precondition hole)",
                    self.file_name
                )
            });
        state.files.documents.remove(&doc_uri);

        // Cascade-delete the page block + all descendants through the same
        // `Mutation::Delete` machinery `ApplyMutation` uses (BFS over
        // parent_id), then re-canonicalize exactly like apply_mutation.rs does.
        let mutation = Mutation::Delete {
            entity: "block".to_string(),
            id: doc_uri.clone(),
        };
        let mut blocks: Vec<Block> = state.domain.block_state.blocks.values().cloned().collect();
        mutation.apply_to(&mut blocks);
        assign_reference_sequences_canonical(&mut blocks);
        let surviving: std::collections::BTreeMap<EntityUri, Block> =
            blocks.into_iter().map(|b| (b.id.clone(), b)).collect();
        state
            .domain
            .block_state
            .block_documents
            .retain(|id, _| surviving.contains_key(id));
        state.domain.block_state.blocks = surviving;
        state.rebuild_profile_tracking();

        state.clear_focus_if_deleted(&doc_uri);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutAppLifecycle> TransitionImpl<ReferenceState, S> for DeleteDocument {
    async fn apply_to_sut(&self, _: &ReferenceState, sut: &mut S) {
        sut.delete_document(&self.file_name).await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for DeleteDocument {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        let watches = state.mcp.active_watches.len();
        let blocks = state.domain.block_state.blocks.len();
        let docs = state.files.documents.len();
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

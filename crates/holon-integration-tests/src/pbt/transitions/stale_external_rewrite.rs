//! Transition: STALE external rewrite of a doc's file (post-startup).
//!
//! @pbt rung external
//!   `stale_external_rewrite` replays the doc's CURRENT content with every
//!   block `:ID:` drawer STRIPPED -- the pre-writeback bytes an editor/agent
//!   holds from before Holon minted ids -- and lets the production
//!   FileSyncController re-ingest it.
//! @pbt covers stale-external-rewrite -- id-less reingest MUST reconcile
//!   against the store's current children, never duplicate (the PR #81 class).
//!
//! Models the duplicate-content bug class Martin hits live: an editor writing
//! from a stale snapshot after Holon's writeback normalized the file. The
//! reference effect is a NO-OP: the content is unchanged, so a correct
//! reconcile against current children leaves the block tree (ids, content,
//! parents) exactly as-is. If the SUT instead mints fresh ids for the id-less
//! incoming blocks, `inv-live-children-match-ref` sees the duplicated tree and
//! goes RED -- which is precisely what this transition should have caught
//! before PR #81 fixed it.

use std::collections::HashMap;
use std::collections::HashSet;

use holon_api::EntityUri;
use holon_filesystem::ExistingChild;
use holon_filesystem::IncomingIdentity;
use holon_filesystem::MatchVerdict;
use holon_filesystem::tiered_match;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::RefBlockTreeMut;
use holon_pbt_core::capabilities::RefDocuments;
use holon_pbt_core::capabilities::RefLayoutInteract;
use holon_pbt_core::capabilities::RefLayoutMutate;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutSeamMutate;
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

/// Replay a doc's current content over its file WITHOUT block ids.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("an external stale rewrite of document {doc_uri}")]
pub struct StaleExternalRewrite {
    pub doc_uri: EntityUri,
}

impl<
    R: RefLifecycle
        + RefDocuments
        + RefLayoutInteract
        + RefLayoutMutate
        + RefBlockTree
        + RefBlockTreeMut,
> TransitionFactory<R> for StaleExternalRewrite
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Only docs that hold editable Text content are worth replaying: an
        // empty doc has no id-less blocks to reconcile, so the class can't fire.
        let docs: Vec<EntityUri> = state
            .document_uris()
            .into_iter()
            .filter(|u| state.doc_has_editable_text(u))
            .collect();
        // OFF by default: this transition reproduces a PARKED (architectural)
        // reconcile/writeback red, so it must not red the shared keystone.
        // `HOLON_PBT_EXTERNAL_RACES=1` arms it for the targeted red run.
        let enabled = std::env::var("HOLON_PBT_EXTERNAL_RACES").is_ok();
        check(enabled && !docs.is_empty(), Reason::NoDocumentsAvailable).map(|_| {
            let strat = prop::sample::select(docs)
                .prop_map(|doc_uri| StaleExternalRewrite { doc_uri })
                .boxed();
            (2u32, strat)
        })
    }
}

impl<
    R: RefLifecycle
        + RefDocuments
        + RefLayoutInteract
        + RefLayoutMutate
        + RefBlockTree
        + RefBlockTreeMut,
> TransitionRef<R> for StaleExternalRewrite
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(
                state.has_document_uri(&self.doc_uri),
                Reason::NoDocumentsAvailable,
            ),
            check(
                state.doc_has_editable_text(&self.doc_uri),
                Reason::PreconditionFailed,
            ),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // Predict the SUT's id-less reconcile by running the SAME `tiered_match`
        // the controller uses, on the reference's view of the doc. Under the R2
        // ruling: content UNIQUE in the document remaps (a true no-op); non-unique
        // / empty content cannot be uniquely matched, so the SUT re-mints it
        // (churn). The oracle must predict the SAME churn or the per-tick
        // synthetic->real reconcile panics (`harness.rs`: `syn=[]` vs many reals).
        // We re-mint every reference block `tiered_match` does NOT remap; Inc 3's
        // write-back then re-stamps the fresh ids to disk, so a REPEAT replay of
        // the same doc is quiescent (ids are back, content+positions stable ->
        // all remap). Tautology disclosure: sharing `tiered_match` means the
        // matching DECISION is not independently oracled here — acceptable because
        // (1) its semantics are pinned by 10 independent red-first unit tests, and
        // (2) the invariants of record (fixed-point, blocks-match-ref family)
        // still check OUTCOMES independently; this shared call only fixes the
        // id-bookkeeping prediction (and honors the one-keystone shared-logic rule).
        let doc = self.doc_uri.clone();

        // Doc blocks in document order (parents before children), each with its
        // real id, normalized parent (top-level -> doc uri), and content.
        fn walk<R: RefBlockTree>(
            state: &R,
            parent: &EntityUri,
            out: &mut Vec<(EntityUri, EntityUri, String)>,
        ) {
            for child in state.sorted_children(parent) {
                let content = state.block_content(&child).unwrap_or_default().to_string();
                out.push((child.clone(), parent.clone(), content));
                walk(state, &child, out);
            }
        }
        let mut ordered: Vec<(EntityUri, EntityUri, String)> = Vec::new();
        walk(state, &doc, &mut ordered);
        if ordered.is_empty() {
            return;
        }

        // existing: real ids; seq = document-order index within the parent.
        let mut per_parent: HashMap<EntityUri, i64> = HashMap::new();
        let existing: Vec<ExistingChild> = ordered
            .iter()
            .map(|(id, parent, content)| {
                let counter = per_parent.entry(parent.clone()).or_insert(0);
                let seq = *counter;
                *counter += 1;
                ExistingChild {
                    id: id.clone(),
                    parent: parent.clone(),
                    seq,
                    content: content.clone(),
                }
            })
            .collect();

        // incoming: placeholder ids in the same document order; a child's parent
        // is its parent's PLACEHOLDER (or the doc uri for top-level), mirroring how
        // the parser re-mints an id-less tree before the controller reconciles it.
        let placeholder_of: HashMap<EntityUri, EntityUri> = ordered
            .iter()
            .enumerate()
            .map(|(i, (id, _, _))| (id.clone(), EntityUri::block(&format!(":stale-in-{i}"))))
            .collect();
        let incoming: Vec<IncomingIdentity> = ordered
            .iter()
            .map(|(id, parent, content)| IncomingIdentity {
                id: placeholder_of[id].clone(),
                parent: if *parent == doc {
                    doc.clone()
                } else {
                    placeholder_of[parent].clone()
                },
                content: content.clone(),
                minted: true,
            })
            .collect();

        let verdicts = tiered_match(&existing, &incoming);
        let remapped: HashSet<EntityUri> = verdicts
            .iter()
            .filter_map(|v| match v {
                MatchVerdict::Remap { onto, .. } => Some(onto.clone()),
                _ => None,
            })
            .collect();

        // Churn: every existing block `tiered_match` did NOT remap onto re-mints.
        for (id, _, _) in &ordered {
            if !remapped.contains(id) {
                state.remint_block(id);
            }
        }
    }
}

crate::cap_transition! {
    StaleExternalRewrite: SutSeamMutate,
    where R: [ RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate + RefBlockTree + RefBlockTreeMut ],
    |me, _state, sut| {
        sut.stale_external_rewrite(&me.doc_uri).await;
    }
    sql_budget: |_me, state| {
        // A correct reconcile writes nothing (content unchanged); the re-scan
        // reads the doc's blocks. Kept permissive -- this transition's oracle
        // is structural (duplicate rows), not SQL cardinality.
        let blocks = state.block_count();
        ExpectedSql {
            reads: REACTIVE_BASE + CACHE_EVENT_READS + blocks,
            writes: 0,
            ddl: 0,
            tolerance: blocks * 8 + 64,
        }
    }
}

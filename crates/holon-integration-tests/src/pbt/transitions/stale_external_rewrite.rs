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

use holon_api::EntityUri;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
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
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct StaleExternalRewrite {
    pub doc_uri: EntityUri,
}

impl<R: RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate> TransitionFactory<R>
    for StaleExternalRewrite
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

impl<R: RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate> TransitionRef<R>
    for StaleExternalRewrite
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

    fn apply_to_ref(&self, _state: &mut R) {
        // NO-OP: replaying the SAME content (only id-less) must reconcile
        // against current children -- ids, content, parents, order all stay.
        // Any SUT-side divergence (duplicate rows, id churn) is therefore a
        // real bug surfaced by `inv-live-children-match-ref`.
    }
}

crate::cap_transition! {
    StaleExternalRewrite: SutSeamMutate,
    where R: [ RefLifecycle + RefDocuments + RefLayoutInteract + RefLayoutMutate ],
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

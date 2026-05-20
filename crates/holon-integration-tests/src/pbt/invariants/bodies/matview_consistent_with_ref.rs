//! `inv-matview-consistent-with-ref` (STRICT).
//!
//! The matview/CDC pipeline feeds the root-layout `data_rows` that the
//! reactive engine renders. This invariant guards those rows against
//! **ghost rows**: ids the matview surfaces that the reference model doesn't
//! know about at all (stale rows left behind by an IVM inconsistency). The
//! reference universe is *every* block (incl. seed + source) plus the layout
//! scaffolding (headline / query-source / render-source) plus the active
//! profile blocks — so any id outside it is a genuine projection bug.
//!
//! # Scope: ghost detection only
//!
//! This check does **not** assert that every expected-visible content block
//! appears in the root layout's `data_rows`. Those rows come from the ROOT
//! LAYOUT query (layout column blocks), not the region-specific content
//! queries, so a "missing content block" there is a hierarchy-level artifact,
//! not a regression. Under-projection of content blocks is covered (Strict)
//! by `inv-block-ids-match-ref` and `inv-live-children-match-ref`, which read
//! the block projection directly. Here we only catch what those can't: a
//! stale row in the root-layout matview itself.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::{EntityUri, RefLayout, SutRenderer};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvMatviewConsistentWithRef;

impl InvMatviewConsistentWithRef {
    pub const ID: InvariantId = InvariantId("inv-matview-consistent-with-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvMatviewConsistentWithRef
where
    R: RefLayout,
    S: SutRenderer,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let data_block_ids = sut.root_data_row_ids().await;

        // An empty matview snapshot carries no rows to compare (engine not
        // warmed up / still loading).
        if data_block_ids.is_empty() {
            return InvariantResult::Skipped("matview snapshot empty".into());
        }

        // Reference universe: every block the ref model tracks (including
        // seed + source) plus layout scaffolding plus profile blocks.
        let ref_block_ids: BTreeSet<EntityUri> = ref_
            .all_block_ids()
            .into_iter()
            .chain(ref_.layout_block_ids())
            .chain(ref_.profile_block_ids())
            .collect();

        let extra: Vec<&EntityUri> = data_block_ids
            .iter()
            .filter(|id| !ref_block_ids.contains(*id))
            .collect();

        if extra.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-matview-consistent-with-ref] IVM MATVIEW GHOST ROW DETECTED!\n  \
             data rows (from root-layout matview): {} ids\n  \
             reference model: {} known ids\n  \
             extra in matview (stale/ghost, not in ref universe): {extra:?}",
            data_block_ids.len(),
            ref_block_ids.len(),
        ))
    }
}

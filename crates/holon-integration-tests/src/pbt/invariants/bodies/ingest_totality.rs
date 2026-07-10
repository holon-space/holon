//! `inv-ingest-totality` — parsed == landed.
//!
//! Every block id the reference expects to have been INGESTED (its non-seed
//! block set, `RefBlockTree::all_non_seed_block_ids`) must be PRESENT in the
//! SUT projection (`SutSqlProjection::all_block_ids`). This is the SUBSET
//! direction (ref ⊆ SUT) of the symmetric `inv-block-ids-match-ref`: a block
//! the reference seeded through the real ingest path but that never landed in
//! the projection is a silent INGEST-DATA-LOSS bug, and this invariant names it
//! as such — distinct from the symmetric-diff message, which reads as generic
//! drift.
//!
//! Motivating regression (dogfood 2026-07-10 P0): a forward same-file
//! `:REQUIRES:` edge to a target parsed LATER in the file hit the
//! `block_requires.required_id` FK at COMMIT, aborting the WHOLE file ingest
//! and silently dropping every block from the requiring one onward. The
//! keystone seeds that corpus (`forward-edge-page.org` →
//! `fe-parent`/`fe-blocked`/ `fe-target`, `WideE2E`) through the real
//! FileSyncController; with the target FK present `fe-blocked`+`fe-target`
//! never land and this invariant fires.
//!
//! Same non-seed filtering as `inv-block-ids-match-ref`
//! (`all_non_seed_block_ids` already excludes seed/`no_parent` docs and the
//! page blocks, which are seed-classified), so it never false-positives on
//! scaffold/page/journals blocks. Binds `RefBlockTree` + `SutSqlProjection`, so
//! it deselects on a draw with no SQL projection (e.g. Loro-only) —
//! ingest totality is a storage-projection property.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvIngestTotality;

impl InvIngestTotality {
    pub const ID: InvariantId = InvariantId("inv-ingest-totality");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvIngestTotality
where
    R: RefBlockTree,
    S: SutSqlProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let expected = ref_.all_non_seed_block_ids();
        let landed: BTreeSet<_> = sut.all_block_ids().await;

        let missing: Vec<&EntityUri> = expected.difference(&landed).collect();
        if missing.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-ingest-totality] INGEST DATA LOSS: {} reference-expected block id(s) were \
             parsed but never landed in the SUT projection (a forward `:REQUIRES:`/`:BLOCKED-BY:` \
             target-FK abort silently drops every block from the offending one onward).\n  \
             missing (expected in ref, absent in SUT): {:?}\n  expected.len={}, landed.len={}",
            missing.len(),
            missing.iter().take(10).collect::<Vec<_>>(),
            expected.len(),
            landed.len(),
        ))
    }
}

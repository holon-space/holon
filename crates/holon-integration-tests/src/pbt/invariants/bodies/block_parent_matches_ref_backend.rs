//! `inv-block-parent-matches-ref/block_raw` — per-block `parent_id` equality
//! between the SUT's write-side `block_raw` snapshot and the reference's
//! [`RefBlockTree`] view.
//!
//! Fills a real gap left by the other block-tree invariants:
//! - `inv-blocks-match-ref/block_raw` compares only `{content, properties}` — it
//!   *excludes* `parent_id` because the wide E2E path remaps doc-level parents
//!   across id spaces (`file:` vs `block:` doc identity), where a raw parent
//!   compare would false-fail;
//! - `inv-no-orphan-blocks` proves the parent *exists*, and
//!   `inv-no-parent-cycles` proves the parent chain *terminates* — neither
//!   catches a block re-parented under a *different but still valid* parent.
//!
//! A pure-memory slice has no doc-id remapping, so `parent_id` is directly
//! comparable; this invariant closes the re-parent-divergence gap there. The
//! reference parent comes through [`RefBlockTree::parent_of`] (owned
//! `Option<EntityUri>`, `None` at a root / sentinel parent).
//!
//! Bound on `RefBlockTree + SutBackend`.

use std::collections::HashMap;

use holon_pbt_core::capabilities::{EntityUri, RefBlockTree, SutBackend};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvBlockParentMatchesRefBackend;

impl InvBlockParentMatchesRefBackend {
    pub const ID: InvariantId = InvariantId("inv-block-parent-matches-ref/block_raw");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlockParentMatchesRefBackend
where
    R: RefBlockTree,
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        // SUT parent per id, normalized: a root / sentinel parent reads as
        // `None`, mirroring `RefBlockTree::parent_of`'s contract.
        let sut_parent: HashMap<EntityUri, Option<EntityUri>> = sut
            .block_raw_snapshot()
            .await
            .into_iter()
            .map(|b| {
                let parent = (!b.parent_id.is_no_parent() && !b.parent_id.is_sentinel())
                    .then_some(b.parent_id);
                (b.id, parent)
            })
            .collect();

        for id in ref_.all_non_seed_block_ids() {
            if crate::pbt::is_synthetic_ref_id(&id) {
                continue;
            }
            let Some(sut_parent_opt) = sut_parent.get(&id) else {
                // Id-set divergence is `inv-blocks-match-ref/block_raw`'s job;
                // this invariant only speaks to parent equality on shared ids.
                continue;
            };
            let ref_parent = ref_.parent_of(&id);
            if &ref_parent != sut_parent_opt {
                return InvariantResult::Fail(format!(
                    "[inv-block-parent-matches-ref/block_raw] block {id} parent diverges:\n  \
                     ref:       {ref_parent:?}\n  block_raw: {sut_parent_opt:?}"
                ));
            }
        }
        InvariantResult::Ok
    }
}

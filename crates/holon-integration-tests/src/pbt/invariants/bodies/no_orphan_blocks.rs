//! `inv-no-orphan-blocks` — structural integrity of the `block` matview
//! mirror: every non-root block must reference a parent that also exists in
//! the snapshot. A dangling parent means the projection lost a node.
//!
//! Pure `SutBackend` self-consistency — no ref, no CDC-lag gate. The
//! deterministic convergence settle (`WideE2E::settle_after_apply` →
//! `converge_projections`) quiesces the matview before the check, so an orphan
//! here is a real projection bug, not a mid-CDC artifact. The former
//! `live_blocks_stale` staleness gate (matview-vs-`block_raw` upstream consult)
//! was proven dead under the settle — its `Lag` arm never fired across a full
//! keystone run with the arm converted to a hard failure — and removed
//! (2026-07-04). Sibling self-check of [`super::no_parent_cycles`].

use std::collections::HashSet;

use holon_pbt_core::capabilities::{EntityUri, SutBackend};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvNoOrphanBlocks;

impl InvNoOrphanBlocks {
    pub const ID: InvariantId = InvariantId("inv-no-orphan-blocks");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNoOrphanBlocks
where
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        // Full matview snapshot (incl. seed/layout blocks — their roots are
        // sentinels skipped below).
        let matview = sut.live_block_snapshot().await;
        let all_ids: HashSet<&EntityUri> = matview.iter().map(|b| &b.id).collect();
        for block in &matview {
            if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                continue;
            }
            if !all_ids.contains(&block.parent_id) {
                return InvariantResult::Fail(format!(
                    "[inv-no-orphan-blocks] orphan block: {} has invalid parent {} \
                     (parent not present in the matview snapshot)",
                    block.id, block.parent_id
                ));
            }
        }
        InvariantResult::Ok
    }
}

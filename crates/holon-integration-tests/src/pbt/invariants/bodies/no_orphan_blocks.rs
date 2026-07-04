//! `inv-no-orphan-blocks` — structural integrity of the `block` matview
//! mirror: every non-root block must reference a parent that also exists in
//! the snapshot. A dangling parent means the projection lost a node.
//!
//! Pure `SutBackend` self-consistency — no ref, no CDC-lag gate. The
//! deterministic convergence settle (`WideE2E::settle_after_apply` →
//! `converge_projections`) quiesces the matview before the check, so an orphan
//! here is a real projection bug, not a mid-CDC artifact.
//!
//! @pbt oracle internal-consistency
//! @pbt covers no-orphan-blocks — every non-root block's parent_id resolves
//!   to a block present in the same matview snapshot
//! @pbt slips-if-removed the projection drops a parent node while keeping its
//!   children; the children become unreachable in the tree and silently
//!   vanish from every view rooted above the lost node
//!
//! The former
//! `live_blocks_stale` staleness gate (matview-vs-`block_raw` upstream consult)
//! was proven dead under the settle — its `Lag` arm never fired across a full
//! keystone run with the arm converted to a hard failure — and removed
//! (2026-07-04). Sibling self-check of [`super::no_parent_cycles`].

use holon_oracles::checks::ParentRow;
use holon_oracles::checks::find_orphans;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

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
        // sentinels the shared check skips). The check body lives in
        // `holon_oracles::checks` — shared with the live debug-build oracle,
        // one implementation, no drift.
        let rows: Vec<ParentRow> = sut
            .live_block_snapshot()
            .await
            .into_iter()
            .map(|b| ParentRow {
                id: b.id,
                parent_id: b.parent_id,
            })
            .collect();
        match find_orphans(&rows).into_iter().next() {
            Some(message) => InvariantResult::Fail(message),
            None => InvariantResult::Ok,
        }
    }
}

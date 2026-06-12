//! `inv-no-orphan-blocks` — structural integrity of the `block` matview
//! mirror: every non-root block must reference a parent that also exists in
//! the snapshot. A dangling parent means the projection lost a node.
//!
//! SUT self-consistency (no per-field ref comparison), but it consults the
//! reference + `block_raw` to reproduce the `live_blocks_stale`
//! gate: when the matview's non-seed id set diverges from the reference yet
//! `block_raw` (the convergent write truth) matches it, the matview is merely
//! mid-CDC — an "orphan" there is a not-yet-seen-parent artifact, not a bug —
//! so the check is `Skipped` (the `live_blocks_stale` CDC-lag computation).

use std::collections::HashSet;

use holon_pbt_core::capabilities::{EntityUri, RefBackend, SutBackend};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvNoOrphanBlocks;

impl InvNoOrphanBlocks {
    pub const ID: InvariantId = InvariantId("inv-no-orphan-blocks");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNoOrphanBlocks
where
    R: RefBackend,
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let seed_block_ids = ref_.seed_block_ids();
        let ref_ids: HashSet<EntityUri> = ref_
            .non_seed_blocks()
            .iter()
            .map(|b| b.id.clone())
            .collect();

        // Full matview snapshot (incl. seed/layout blocks — their roots are
        // sentinels skipped below). The orphan check runs over this set.
        let matview = sut.live_block_snapshot().await;
        let backend_ids: HashSet<EntityUri> = matview
            .iter()
            .map(|b| b.id.clone())
            .filter(|id| !seed_block_ids.contains(id))
            .collect();

        // Staleness gate (reproduces `live_blocks_stale`): matview diverges
        // from the reference but `block_raw` matches it → CDC lag → Skip.
        if backend_ids != ref_ids {
            // `block_raw` id set, derived from the same write-side snapshot the
            // other structural invariants read (`block_raw_snapshot`) rather than
            // a separate `SutSqlProjection::all_block_ids` query — identical id
            // set (both are `block_raw`), one fewer cap dependency, so this body
            // needs only `SutBackend`.
            let block_raw_ids: HashSet<EntityUri> = sut
                .block_raw_snapshot()
                .await
                .into_iter()
                .map(|b| b.id)
                .filter(|id| !seed_block_ids.contains(id))
                .collect();
            if block_raw_ids == ref_ids {
                return InvariantResult::Skipped(
                    "[inv-no-orphan-blocks] matview mirror lagging block_raw (CDC race) — \
                     an orphan here is a not-yet-seen-parent artifact, not a bug"
                        .to_string(),
                );
            }
            // block_raw also disagrees → real pipeline bug; the matview store
            // invariant fails on that separately. Proceed with the orphan
            // check (`live_blocks_stale` is false here).
        }

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

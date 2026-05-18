//! Phase 7 — `inv-block-ids-match-ref` (FUNCTIONAL, NEW INVARIANT).
//!
//! Set-equality check between the SQL projection's block ids and the
//! reference model's non-seed block ids. Bound only on `SutSqlProjection`
//! + `RefBlockTree` — runs in any slice with a storage backing.
//!
//! Catches the bug class `inv-backend-blocks-match-ref` was designed
//! for (block-tree drift between SQL projection and ref model) but at
//! a coarser granularity. The deeper field-level comparison stays in
//! the inline `inv-backend-blocks-match-ref` body in
//! `sut.rs::check_invariants_async` for now; that one is the
//! finer-grained safety net.
//!
//! Why this rephrasing matters: set-equality catches the most common
//! drift symptoms (block exists in ref but not SQL, or vice versa)
//! without needing `assert_blocks_equivalent` plumbing. Runs in the
//! Phase 8 storage_consistency_pbt slice with no extra wiring.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::{RefBlockTree, SutSqlProjection};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvBlockIdsMatchRef;

impl InvBlockIdsMatchRef {
    pub const ID: InvariantId = InvariantId("inv-block-ids-match-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlockIdsMatchRef
where
    R: RefBlockTree,
    S: SutSqlProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    fn mode(&self) -> RunMode {
        // Strict in the storage slice; in the wide PBT, the CDC-lag
        // classifier downgrades it via the surrounding invariant
        // dispatcher when live_blocks_stale (proxy.cdc_in_flight_cached).
        RunMode::Strict
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let ref_ids = ref_.all_non_seed_block_ids();
        let sql_ids: BTreeSet<_> = sut.all_block_ids().await;

        let missing_in_sql: Vec<&String> = ref_ids.difference(&sql_ids).collect();
        let extra_in_sql: Vec<&String> = sql_ids.difference(&ref_ids).collect();

        if missing_in_sql.is_empty() && extra_in_sql.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-block-ids-match-ref] block id set diverges between ref model \
             and SQL projection.\n  \
             missing in SQL ({}): {:?}\n  \
             extra in SQL ({}): {:?}\n  \
             ref_ids.len={}, sql_ids.len={}",
            missing_in_sql.len(),
            missing_in_sql.iter().take(10).collect::<Vec<_>>(),
            extra_in_sql.len(),
            extra_in_sql.iter().take(10).collect::<Vec<_>>(),
            ref_ids.len(),
            sql_ids.len(),
        ))
    }
}

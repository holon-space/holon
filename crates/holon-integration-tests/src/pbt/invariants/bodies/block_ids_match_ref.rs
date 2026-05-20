//! `inv-block-ids-match-ref`.
//!
//! Set-equality check between the SQL projection's block ids and the
//! reference model's non-seed block ids. Bound only on `SutSqlProjection`
//! + `RefBlockTree` — runs in any slice with a storage backing.
//!
//! Catches block-tree drift between SQL projection and ref model at a
//! coarse granularity; the deeper field-level comparison lives in
//! `inv-blocks-match-ref/matview`.
//!
//! Set-equality catches the most common drift symptoms (block exists in ref
//! but not SQL, or vice versa) without needing `assert_blocks_equivalent`
//! plumbing. Runs in the storage_consistency_pbt slice with no extra wiring.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::{EntityUri, RefBlockTree, SutSqlProjection};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

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

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let ref_ids = ref_.all_non_seed_block_ids();
        let sql_ids: BTreeSet<_> = sut.all_block_ids().await;

        let missing_in_sql: Vec<&EntityUri> = ref_ids.difference(&sql_ids).collect();
        let extra_in_sql: Vec<&EntityUri> = sql_ids.difference(&ref_ids).collect();

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

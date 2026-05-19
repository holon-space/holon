//! `inv-block-content-matches-ref` — per-block `content` column equality
//! between the SQL projection and the reference model.
//!
//! Complements `inv-block-ids-match-ref` (set-equality only) by catching
//! divergences that share the same id set but differ in stored content.
//! Specifically targets the May-2026 SqlOnly SplitBlock content-routing
//! bug: production assigns the whole pre-split text to the new block and
//! leaves the original block's `content` column empty, where the ref model
//! correctly splits prefix to original / suffix to new.
//!
//! Bound on `RefBlockTree + SutSqlProjection`. Runs in any slice that
//! enables the storage backing.

use holon_pbt_core::capabilities::{RefBlockTree, SutSqlProjection};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvBlockContentMatchesRef;

impl InvBlockContentMatchesRef {
    pub const ID: InvariantId = InvariantId("inv-block-content-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvBlockContentMatchesRef
where
    R: RefBlockTree,
    S: SutSqlProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    fn mode(&self) -> RunMode {
        RunMode::Strict
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        for id in ref_.all_non_seed_block_ids() {
            // Synthetic ref-side ids (`block::split-N`, `block::bulk-N-M`) get
            // remapped to UUIDs production-side; the SUT has no row at the
            // synthetic id. Skip — the slice's role is content equality on
            // stable ids; the wider PBT's synthetic-id-mapping does the
            // cross-side reconciliation for these.
            if id.contains("::split-") || id.contains("::bulk-") {
                continue;
            }
            let ref_content = match ref_.block_content(&id) {
                Some(c) => c.to_string(),
                None => continue,
            };
            let sql_content = match sut.block_content(&id).await {
                Some(c) => c,
                None => {
                    return InvariantResult::Fail(format!(
                        "[inv-block-content-matches-ref] block {id} present in ref \
                         but missing from SQL projection (ref content = {ref_content:?})"
                    ));
                }
            };
            if ref_content != sql_content {
                return InvariantResult::Fail(format!(
                    "[inv-block-content-matches-ref] block {id} content diverges:\n  \
                     ref: {ref_content:?}\n  sql: {sql_content:?}"
                ));
            }
        }
        InvariantResult::Ok
    }
}

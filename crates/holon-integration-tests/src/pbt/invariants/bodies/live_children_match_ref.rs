//! `inv-live-children-match-ref` (FUNCTIONAL).
//!
//! Per-parent sibling-order equality between the SQL projection and the
//! reference model. For every parent of a non-seed block, the projection's
//! `sorted_children` (ordered by `sort_key`, the authoritative fractional
//! index) must equal the ref model's `sorted_children` (document order),
//! restricted to non-seed blocks.
//!
//! This is the functional successor to the deferred skeleton: it became
//! cheap once `RefBlockTree::sorted_children` (ref side) and
//! `SutSqlProjection::sorted_children` (SQL `ORDER BY sort_key`) existed.
//!
//! Bug class caught: org-ingested blocks whose Loro fractional index never
//! reaches SQL `sort_key` (left at the default `"A0"`) and therefore
//! mis-sort against moved siblings carrying a real fi — the
//! projection-totality gap. Runs in any slice that is `RefBlockTree` +
//! `SutSqlProjection` (e.g. `org_create_ordering_pbt`).

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::{EntityUri, RefBlockTree, SutSqlProjection};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub struct InvLiveChildrenMatchRef;

impl InvLiveChildrenMatchRef {
    pub const ID: InvariantId = InvariantId("inv-live-children-match-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLiveChildrenMatchRef
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
        let non_seed = ref_.all_non_seed_block_ids();

        // Parents to verify: the distinct parents of every non-seed block.
        let mut parents: BTreeSet<EntityUri> = BTreeSet::new();
        for id in &non_seed {
            if let Some(parent) = ref_.parent_of(id) {
                parents.insert(parent);
            }
        }

        for parent in &parents {
            let ref_children: Vec<EntityUri> = ref_
                .sorted_children(parent)
                .into_iter()
                .filter(|c| non_seed.contains(c))
                .collect();
            let sql_children: Vec<EntityUri> = sut
                .sorted_children(parent)
                .await
                .into_iter()
                .filter(|c| non_seed.contains(c))
                .collect();

            if ref_children != sql_children {
                return InvariantResult::Fail(format!(
                    "[inv-live-children-match-ref] sibling order diverges under parent \
                     {parent}.\n  ref (doc order): {ref_children:?}\n  \
                     sql (ORDER BY sort_key): {sql_children:?}"
                ));
            }
        }

        InvariantResult::Ok
    }
}

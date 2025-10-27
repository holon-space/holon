//! `inv-live-children-match-ref`.
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

use holon_pbt_core::capabilities::{EntityUri, RefBlockTree, SutLoroLog, SutSqlProjection};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

use crate::pbt::invariants::block_compare::compare_sibling_order;

pub struct InvLiveChildrenMatchRef;

impl InvLiveChildrenMatchRef {
    pub const ID: InvariantId = InvariantId("inv-live-children-match-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLiveChildrenMatchRef
where
    R: RefBlockTree,
    S: SutSqlProjection + SutLoroLog,
{
    fn id(&self) -> InvariantId {
        Self::ID
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

            // Shared exemption rule: render-artifact groups may reorder
            // relative to the ref, but membership must match.
            if let Err(msg) = compare_sibling_order(
                "inv-live-children-match-ref",
                parent,
                &ref_children,
                &sql_children,
                |c| ref_.is_order_exempt_sibling(c),
            ) {
                return InvariantResult::Fail(msg);
            }

            // Even where the ref's order is exempt, the two SUT stores must
            // agree WITH EACH OTHER: SQL `sort_key` derives from the Loro
            // fractional index, so a Loro-vs-SQL order disagreement is a
            // projection bug regardless of the ref exemption. `None` ⇒ Loro
            // not wired / parent absent: nothing to cross-check.
            if let Some(loro_raw) = sut.loro_children_of(parent.as_str()).await {
                let non_seed_strs: BTreeSet<String> =
                    non_seed.iter().map(|id| id.to_string()).collect();
                let loro_children: Vec<String> = loro_raw
                    .into_iter()
                    .filter(|c| non_seed_strs.contains(c))
                    .collect();
                let sql_children_strs: Vec<String> =
                    sql_children.iter().map(|c| c.to_string()).collect();
                if loro_children != sql_children_strs {
                    return InvariantResult::Fail(format!(
                        "[inv-live-children-match-ref] Loro and SQL disagree on sibling \
                         order under parent {parent} (no exemption applies — sort_key \
                         derives from the Loro fractional index).\n  \
                         loro (fractional-index order): {loro_children:?}\n  \
                         sql (ORDER BY sort_key): {sql_children_strs:?}"
                    ));
                }
            }
        }

        InvariantResult::Ok
    }
}

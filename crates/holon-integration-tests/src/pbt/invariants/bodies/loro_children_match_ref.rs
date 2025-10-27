//! `inv-loro-children-match-ref`.
//!
//! Per-parent sibling-order equality between the **Loro** adapter and the
//! reference model. For every parent of a non-seed block, the order Loro
//! reports (`loro_children_of`, sorted by the tree's internal fractional
//! index) must equal the ref model's `sorted_children` (document order),
//! restricted to non-seed blocks.
//!
//! This is the Loro-side companion to `inv-live-children-match-ref` (which
//! checks the SQL projection). Together they are the agreed-upon replacement
//! for the old cross-backend `sort_key` *byte*-equality check, retired in
//! Phase 8 (ADR 0005): the ordering encoding is now adapter-internal and never
//! compared byte-for-byte across adapters. Instead each adapter must agree with
//! the reference on the *order* its internal encoding produces — and, since the
//! per-adapter order is derived by sorting on that encoding, this is exactly
//! the "internal encoding stays monotone w.r.t. `children_of`" guarantee the
//! ADR requires.
//!
//! `loro_children_of` returns `None` when Loro is not wired (or the parent is
//! absent from the tree); those parents are skipped, so this invariant is inert
//! in SqlOnly slices and meaningful in Loro slices.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use crate::pbt::invariants::block_compare::compare_sibling_order;

pub struct InvLoroChildrenMatchRef;

impl InvLoroChildrenMatchRef {
    pub const ID: InvariantId = InvariantId("inv-loro-children-match-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLoroChildrenMatchRef
where
    R: RefBlockTree,
    S: SutLoroLog,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let non_seed = ref_.all_non_seed_block_ids();
        let non_seed_strs: BTreeSet<String> = non_seed.iter().map(|id| id.to_string()).collect();

        // Parents to verify: the distinct parents of every non-seed block.
        let mut parents: BTreeSet<EntityUri> = BTreeSet::new();
        for id in &non_seed {
            if let Some(parent) = ref_.parent_of(id) {
                parents.insert(parent);
            }
        }

        for parent in &parents {
            // None ⇒ Loro not wired, or the parent isn't in the tree: skip.
            let Some(loro_raw) = sut.loro_children_of(parent.as_str()).await else {
                continue;
            };

            let ref_children: Vec<EntityUri> = ref_
                .sorted_children(parent)
                .into_iter()
                .filter(|c| non_seed.contains(c))
                .collect();
            let loro_children: Vec<String> = loro_raw
                .into_iter()
                .filter(|c| non_seed_strs.contains(c))
                .collect();

            let ref_children_strs: Vec<String> =
                ref_children.iter().map(|c| c.to_string()).collect();

            // Shared render-artifact exemption rule (block_compare.rs):
            // intra-group order may differ from the ref, membership may not.
            // The exemption predicate keys by id string via the ref lookup.
            let exempt_strs: BTreeSet<String> = ref_children
                .iter()
                .filter(|c| ref_.is_order_exempt_sibling(c))
                .map(|c| c.to_string())
                .collect();
            if let Err(msg) = compare_sibling_order(
                "inv-loro-children-match-ref",
                parent,
                &ref_children_strs,
                &loro_children,
                |c| exempt_strs.contains(c.as_str()),
            ) {
                return InvariantResult::Fail(msg);
            }
        }

        InvariantResult::Ok
    }
}

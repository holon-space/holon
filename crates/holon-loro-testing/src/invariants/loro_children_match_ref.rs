//! `inv-loro-children-match-ref` — `SutLoroLog` + `RefBlockTree`. Per-parent
//! sibling-order equality between the **Loro** adapter (the tree's fractional
//! index, via `loro_children_of`) and the reference model's document order
//! (`sorted_children`), restricted to non-seed blocks. The Loro-side companion
//! to `inv-live-children-match-ref` (SQL projection). Real teeth in the Loro
//! slice: a CRDT reorder bug surfaces as a per-parent order divergence against
//! the independent reference.
//!
//! Retired the old cross-backend `sort_key` *byte*-equality check (ADR 0005):
//! ordering encoding is adapter-internal and never compared byte-for-byte
//! across adapters; instead each adapter must agree with the reference on the
//! *order* its internal encoding produces.

use std::collections::BTreeSet;

use holon_pbt_core::RunMode;
use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::RefBlockTree;
use holon_pbt_core::capabilities::SutLoroLog;
use holon_pbt_core::composition::BridgedInvariant;
use holon_pbt_core::composition::CapId;
use holon_pbt_core::composition::CapInvariant;
use holon_pbt_core::composition::Needs;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;
use holon_pbt_core::sibling_order::compare_sibling_order;

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

            // Order is compared EXACTLY: the render-artifact exemption was
            // removed once the reference model was taught to reproduce the
            // store's true post-round-trip sibling order (`parse_order_rank`:
            // `Source < Image < Text`).
            if let Err(msg) = compare_sibling_order(
                "inv-loro-children-match-ref",
                parent,
                &ref_children_strs,
                &loro_children,
            ) {
                return InvariantResult::Fail(msg);
            }
        }

        InvariantResult::Ok
    }
}

pub fn wire() -> Box<dyn CapInvariant> {
    Box::new(BridgedInvariant::new(
        InvLoroChildrenMatchRef,
        RunMode::Strict,
        Needs {
            sut_present: vec![CapId::of::<dyn SutLoroLog>()],
            sut_absent: Vec::new(),
            ref_present: vec![CapId::of::<dyn RefBlockTree>()],
        },
    ))
}

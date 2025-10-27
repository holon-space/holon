//! `inv-no-parent-cycles` — ADR-0004 domain invariant.
//!
//! Domain rule: the block parent relation is acyclic — following `parent_id`
//! from any block eventually reaches a root (no-parent / sentinel) without
//! revisiting a node. Cycles are rejected at mutation time
//! (`BlockMutation::validate` / Loro `tree.mov`), so a cycle reaching the SUT's
//! projection means a write path bypassed that guard.
//!
//! `inv-no-orphan-blocks` proves every parent *exists*; this proves the parent
//! chain *terminates*. Together they certify a well-formed forest. Read from
//! the convergent write-side truth (`block_raw_snapshot`) after the shared
//! settle, so there is no CDC-lag window to tolerate.

use std::collections::HashMap;
use std::collections::HashSet;

use holon_pbt_core::capabilities::EntityUri;
use holon_pbt_core::capabilities::SutBackend;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvNoParentCycles;

impl InvNoParentCycles {
    pub const ID: InvariantId = InvariantId("inv-no-parent-cycles");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvNoParentCycles
where
    S: SutBackend,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let snapshot = sut.block_raw_snapshot().await;
        let parents: HashMap<EntityUri, EntityUri> = snapshot
            .iter()
            .filter(|b| !b.parent_id.is_no_parent() && !b.parent_id.is_sentinel())
            .map(|b| (b.id.clone(), b.parent_id.clone()))
            .collect();

        for start in parents.keys() {
            let mut seen: HashSet<&EntityUri> = HashSet::new();
            let mut current = start;
            while let Some(parent) = parents.get(current) {
                if !seen.insert(current) {
                    return InvariantResult::Fail(format!(
                        "[inv-no-parent-cycles] parent cycle detected walking up from {start}: \
                         revisited {current} (chain re-enters a node instead of terminating at a \
                         root)",
                    ));
                }
                current = parent;
            }
        }
        InvariantResult::Ok
    }
}

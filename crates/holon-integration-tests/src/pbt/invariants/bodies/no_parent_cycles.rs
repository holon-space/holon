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

use holon_oracles::checks::ParentRow;
use holon_oracles::checks::find_parent_cycles;
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
        // Check body lives in `holon_oracles::checks` — shared with the live
        // debug-build oracle, one implementation, no drift.
        let rows: Vec<ParentRow> = sut
            .block_raw_snapshot()
            .await
            .into_iter()
            .map(|b| ParentRow {
                id: b.id,
                parent_id: b.parent_id,
            })
            .collect();
        match find_parent_cycles(&rows).into_iter().next() {
            Some(message) => InvariantResult::Fail(message),
            None => InvariantResult::Ok,
        }
    }
}

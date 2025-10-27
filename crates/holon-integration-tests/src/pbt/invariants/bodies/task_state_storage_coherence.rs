//! `inv-task-state-storage-coherence`.
//!
//! Cross-checks `task_state` between `block_raw.properties` (SQL) and
//! the Loro tag projection, catching Loro↔SQL desync at the data layer
//! before any rendering bug surfaces.
//!
//! Bound: `S: SutSqlProjection + SutLoroTaskState`. No ref-side bound —
//! both sides come from the SUT.
//!
//! ## Deferral note
//!
//! The Loro side (`SutLoroTaskState::loro_task_state_of`) is `unimplemented!`
//! on `E2ESut`: projecting task_state from Loro tags requires exposing the
//! LoroSyncController's tag map through `TestContext`, which is not yet wired.
//! The invariant body is defined so it compiles and the ID is reserved; at
//! runtime it will panic with the `unimplemented!` message if invoked against
//! `E2ESut`, so it is NOT added to any live PBT runner until the plumbing
//! lands.
//!
//! ## Mismatch policy
//!
//! Any disagreement between SQL and Loro — including `None` on one side
//! and `Some` on the other — is treated as a failure. Both sides must
//! agree on the presence AND value of `task_state`. This strict policy
//! maximises bug catch rate; if CDC lag causes false positives in the
//! wide PBT, the surrounding `live_blocks_stale` classifier can downgrade
//! to WARN just as it does for `inv1`/`inv3`.

use std::marker::PhantomData;

use holon_pbt_core::capabilities::SutLoroTaskState;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvTaskStateStorageCoherence<R>(pub PhantomData<R>);

impl<R> InvTaskStateStorageCoherence<R> {
    pub const ID: InvariantId = InvariantId("inv-task-state-storage-coherence");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvTaskStateStorageCoherence<R>
where
    S: SutSqlProjection + SutLoroTaskState,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        let all_ids = sut.all_block_ids().await;
        let mut mismatches: Vec<String> = Vec::new();

        for id in &all_ids {
            let sql_state = sut.block_task_state(id).await;
            let loro_state = sut.loro_task_state_of(id.as_str()).await;

            if sql_state != loro_state {
                mismatches.push(format!("  {id}: sql={:?} loro={:?}", sql_state, loro_state));
            }
        }

        if mismatches.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-task-state-storage-coherence] {count} block(s) have diverging task_state \
             between block_raw SQL and Loro projection.\n{details}",
            count = mismatches.len(),
            details = mismatches
                .iter()
                .take(10)
                .cloned()
                .collect::<Vec<_>>()
                .join("\n"),
        ))
    }
}

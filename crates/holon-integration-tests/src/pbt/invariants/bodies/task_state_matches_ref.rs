//! `inv-task-state-matches-ref`.
//!
//! @pbt oracle correspondence — every block's `task_state` in the SQL
//!   projection (`SutSqlProjection::block_task_state`) equals the reference's
//!   (`RefTaskState::task_state_of`).
//! @pbt covers task-state-drift — the SUT holds a task state the model does not
//!   predict, or misses one it does
//! @pbt slips-if-removed a live authoring gesture's task-state effect goes
//!   unobserved IN THE SqlOnly ARM: the sibling
//!   `inv-task-state-storage-coherence` needs `SutLoroTaskState` and so
//!   deselects in the SqlOnly arm — and `inv-blocks-match-ref/block_raw`
//!   reports whichever facet diverges FIRST, so a promotion (which also moves
//!   `Content`) reads there as a whole-block dump, not as a task-state fact.
//!
//! Bound: `R: RefTaskState`, `S: SutSqlProjection` — the SQL projection is the
//! one block store present in BOTH the Loro and the SqlOnly arm, so this
//! invariant is mode-independent by construction.

use std::marker::PhantomData;

use holon_pbt_core::capabilities::RefTaskState;
use holon_pbt_core::capabilities::SutSqlProjection;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvTaskStateMatchesRef<R>(pub PhantomData<R>);

impl<R> InvTaskStateMatchesRef<R> {
    pub const ID: InvariantId = InvariantId("inv-task-state-matches-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvTaskStateMatchesRef<R>
where
    R: RefTaskState,
    S: SutSqlProjection,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let mut mismatches: Vec<String> = Vec::new();
        for id in &sut.all_block_ids().await {
            let sut_state = sut.block_task_state(id).await;
            let ref_state = ref_.task_state_of(id);
            if sut_state != ref_state {
                mismatches.push(format!(
                    "  {id}: expected task_state={ref_state:?} (reference), actual \
                     task_state={sut_state:?} (SUT)"
                ));
            }
        }

        if mismatches.is_empty() {
            return InvariantResult::Ok;
        }

        InvariantResult::Fail(format!(
            "[inv-task-state-matches-ref] {count} block(s) have a task_state the reference model \
             does not predict.\n{details}",
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

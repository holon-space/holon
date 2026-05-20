//! `inv-active-watches-match-ref` — the set of registered watch query ids on
//! the SUT equals the reference's. The watch *rows* are checked separately by
//! `inv-watch-rows-match-ref`; this is just the subscription-set agreement
//! (the watches half of the view/watches check).
//!
//! Fully cap-covered, no new cap: `SutWatchRows::watch_query_ids()` reads
//! `ctx.ui_model.keys()`, which `add_watch`/`remove_watch` keep identical to
//! `ctx.active_watches.keys()`; `RefWatches::active_watch_ids()` is the keys of
//! `ReferenceState::active_watches`.

use std::collections::BTreeSet;

use holon_pbt_core::capabilities::{RefWatches, SutWatchRows};
use holon_pbt_core::invariant::{Invariant, InvariantId, InvariantResult};

pub struct InvActiveWatchesMatchRef;

impl InvActiveWatchesMatchRef {
    pub const ID: InvariantId = InvariantId("inv-active-watches-match-ref");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvActiveWatchesMatchRef
where
    R: RefWatches,
    S: SutWatchRows,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, ref_: &R, sut: &S) -> InvariantResult {
        let sut_ids: BTreeSet<String> = sut.watch_query_ids().await.into_iter().collect();
        let ref_ids: BTreeSet<String> = ref_.active_watch_ids().into_iter().collect();
        if sut_ids == ref_ids {
            return InvariantResult::Ok;
        }
        let missing: Vec<&String> = ref_ids.difference(&sut_ids).collect();
        let spurious: Vec<&String> = sut_ids.difference(&ref_ids).collect();
        InvariantResult::Fail(format!(
            "[inv-active-watches-match-ref] watch sets diverged\n  missing on SUT: {missing:?}\n  \
             spurious on SUT: {spurious:?}"
        ))
    }
}

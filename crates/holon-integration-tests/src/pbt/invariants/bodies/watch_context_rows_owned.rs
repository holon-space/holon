//! `inv-watch-context-rows-owned` — every `watch_context` row belongs to a
//! watch that is still live.
//!
//! The shared keyed views maintain one subtree per membership row, so a row
//! whose watch has ended is not a cosmetic leak: it keeps that subtree
//! incrementally maintained on every later write, for nobody. The property is
//! asserted on STATE — the rows the database holds against the watches the
//! engine holds — because the ways a row is orphaned are mostly silent: a
//! delete that is never issued, or one that matches nothing, reports no error
//! to count.
//!
//! @pbt oracle internal-consistency
//! @pbt covers watch-context-rows-owned — no `watch_context` row outlives the
//!   watch that registered it
//! @pbt slips-if-removed a watch ends without releasing its row; every later
//!   write re-derives that subtree's closure for a panel that is gone, and
//!   the cost compounds with every re-render

use holon_pbt_core::capabilities::SutWatchContext;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

/// Carries the opens count of the previous check, so the number reported is
/// the transition's own. A FIELD, not a static: catalogs of parallel cases
/// live in one process, and a shared counter would attribute each case's
/// re-opens to whichever case read it last.
#[derive(Default)]
pub struct InvWatchContextRowsOwned {
    opens_at_last_check: std::sync::atomic::AtomicU64,
}

impl InvWatchContextRowsOwned {
    pub const ID: InvariantId = InvariantId("inv-watch-context-rows-owned");
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvWatchContextRowsOwned
where
    S: SutWatchContext,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        // D163.a visibility: with a stable watch key the snapshot reads dedup
        // back down, so no budget shows how many watches a re-render
        // re-opened. `saturating_sub` because the engine under a rebooting
        // SUT starts its count again.
        let opens = sut.watch_context_opens().await;
        let since_last = opens.saturating_sub(
            self.opens_at_last_check
                .swap(opens, std::sync::atomic::Ordering::Relaxed),
        );
        let held = sut.watched_places().await;
        eprintln!(
            "[inv-watch-context-rows-owned] watch_opens={since_last} (cumulative {opens}) \
             places={held:?}"
        );

        let unowned = sut.unowned_watch_context_rows().await;
        if unowned.is_empty() {
            return InvariantResult::Ok;
        }
        InvariantResult::Fail(format!(
            "{} membership row(s) outlived their watches: {}",
            unowned.len(),
            unowned.join("; ")
        ))
    }
}

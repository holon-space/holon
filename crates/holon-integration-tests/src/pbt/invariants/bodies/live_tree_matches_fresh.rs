//! `inv-live-tree-matches-fresh`.
//!
//! The persistent live ViewModel tree (the collection driver's `set_data`
//! path, mirroring the GPUI frontend) must agree with a fresh interpretation
//! of the same data rows. The fresh tree always reflects current data, so it
//! can't catch bugs where `set_data` fails to propagate updated props to child
//! widgets (`state_toggle`, `editable_text`, …) — only the live tree can.
//!
//! Reads [`SutFrontendEmissions::live_vs_fresh_tree_diff`], which keeps the
//! `ReactiveEngine`/`HeadlessLiveTree` coupling SUT-side and returns:
//! `None` when the comparison can't run yet (Skipped), `Some(vec![])` when the
//! trees agree, and `Some(diffs)` listing the per-item prop divergences.

use holon_pbt_core::capabilities::SutFrontendEmissions;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

pub struct InvLiveTreeMatchesFresh;

impl InvLiveTreeMatchesFresh {
    pub const ID: InvariantId = InvariantId("inv-live-tree-matches-fresh");
    const LABEL: &'static str = "inv-live-tree-matches-fresh";
}

#[allow(async_fn_in_trait)]
impl<R, S> Invariant<R, S> for InvLiveTreeMatchesFresh
where
    S: SutFrontendEmissions,
{
    fn id(&self) -> InvariantId {
        Self::ID
    }

    async fn check(&self, _: &R, sut: &S) -> InvariantResult {
        match sut.live_vs_fresh_tree_diff().await {
            None => InvariantResult::Skipped(
                "live tree not ready (engine/main-panel loading, no rows, or no item template)"
                    .into(),
            ),
            Some(diffs) if diffs.is_empty() => InvariantResult::Ok,
            Some(diffs) => InvariantResult::Fail(format!(
                "[{label}] LIVE tree diverges from FRESH tree! The collection driver's set_data \
                 path produces different props than fresh interpretation. Child widgets see stale \
                 data in the GPUI frontend.\n\nDiffs ({n}):\n{body}",
                label = Self::LABEL,
                n = diffs.len(),
                body = diffs.join("\n"),
            )),
        }
    }
}

//! Composed-slice provider + body for `inv-settle-budget`.
//!
//! The measurement point is the harness's per-transition timed window
//! (`ComposedSut::apply`): dispatch through the production pipeline
//! (`apply_transition`) plus the 3-projection convergence settle
//! (`settle_after_apply`) — the same window the `holon_latency`
//! `stage=action_total` event already reports. The harness hands it here via
//! [`SettleLatencyLifecycle::note_settle`]; the invariant reads it back
//! through [`SettleLatency::last_settle`].
//!
//! Ref-less, like [`crate::pbt::composed::span_metrics::ComposedBudget`]: a
//! duration is not something the reference model can predict, so there is
//! nothing to compare against on the ref axis — the oracle is the SLO.
//!
//! `CapMap` is single-threaded (`!Send`/`!Sync`, accessed under the harness's
//! `block_on`), so plain `RefCell` interior mutability suffices.

use std::cell::RefCell;
use std::time::Duration;

use holon_pbt_core::composition::CapMap;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantId;
use holon_pbt_core::invariant::InvariantResult;

use crate::pbt::invariants::bodies::settle_budget::InvSettleBudget;
use crate::pbt::invariants::bodies::settle_budget::SettleSample;
use crate::pbt::invariants::bodies::settle_budget::verdict;

/// Read cap: the last transition's measured settle duration.
#[holon_macros::capmap_adapter]
pub trait SettleLatency {
    fn last_settle(&self) -> Option<SettleSample>;
}

/// Lifecycle cap the harness drives once per transition, right after the
/// settle completes.
#[holon_macros::capmap_adapter]
pub trait SettleLatencyLifecycle {
    fn note_settle(&self, action: &str, elapsed: Duration);
}

#[derive(Default)]
pub struct ComposedSettleLatency {
    last: RefCell<Option<SettleSample>>,
}

impl ComposedSettleLatency {
    pub fn new() -> Self {
        Self::default()
    }
}

impl SettleLatencyLifecycle for ComposedSettleLatency {
    fn note_settle(&self, action: &str, elapsed: Duration) {
        *self.last.borrow_mut() = Some(SettleSample {
            action: action.to_string(),
            elapsed,
        });
    }
}

impl SettleLatency for ComposedSettleLatency {
    fn last_settle(&self) -> Option<SettleSample> {
        self.last.borrow().clone()
    }
}

/// The dispatched body (same id as [`InvSettleBudget`]). The harness runs an
/// initial `check_invariants` BEFORE the first transition, so on that tick
/// there is no sample and the verdict is trivially `Ok`.
pub struct InvComposedSettleBudget;

#[allow(async_fn_in_trait)]
impl Invariant<CapMap, CapMap> for InvComposedSettleBudget {
    fn id(&self) -> InvariantId {
        InvSettleBudget::ID
    }

    async fn check(&self, _: &CapMap, sut: &CapMap) -> InvariantResult {
        verdict(sut.last_settle().as_ref())
    }
}

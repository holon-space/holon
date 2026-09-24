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
use std::time::SystemTime;

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
/// settle completes. `opened` is when the transition's timed window opened:
/// the transition is classed by the ops dispatched from then on.
#[holon_macros::capmap_adapter]
pub trait SettleLatencyLifecycle {
    fn note_settle(&self, action: &str, opened: SystemTime, elapsed: Duration);
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
    fn note_settle(&self, action: &str, opened: SystemTime, elapsed: Duration) {
        *self.last.borrow_mut() = Some(SettleSample {
            action: action.to_string(),
            elapsed,
            class: crate::pbt::invariants::bodies::settle_budget::settle_class(
                &crate::pbt::net_cap::fired_operations_since(opened),
            ),
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

#[cfg(test)]
mod tests {
    use holon::api::operation_dispatcher::OpClass;

    use super::*;
    use crate::test_tracing::SpanCollector;
    use crate::test_tracing::begin_test_scope;

    fn dispatch_span(entity: &str, op: &str) {
        tracing::info_span!(
            "dispatcher.execute_operation",
            "operation.entity" = entity,
            "operation.name" = op
        )
        .in_scope(|| {});
    }

    #[test]
    fn a_transition_is_classed_only_by_the_ops_it_dispatched() {
        SpanCollector::global();
        begin_test_scope();
        let settle = ComposedSettleLatency::new();

        let rebuild_opened = SystemTime::now();
        dispatch_span("*", "rebuild_views");
        settle.note_settle("RebuildViews", rebuild_opened, Duration::from_millis(900));
        assert_eq!(
            settle.last_settle().expect("noted").class,
            OpClass::Maintenance
        );

        let reboot_opened = SystemTime::now();
        settle.note_settle("Reboot", reboot_opened, Duration::from_millis(795));
        assert_eq!(
            settle.last_settle().expect("noted").class,
            OpClass::Interaction,
            "a Reboot dispatches no op, so the rebuild_views before it must not class it"
        );
    }
}

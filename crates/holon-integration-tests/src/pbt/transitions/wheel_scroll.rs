//! Transition: scroll a wheel over a scroll region (Inc D).
//!
//! @pbt rung input-pipeline
//!   `scroll_over` drives the production `UserDriver` wheel path (real
//!   `ScrollWheelEvent` at the element centre).
//! @pbt covers wheel-scroll — a scroll-wheel gesture over the outer list or the
//!   sticky footer, source-routed by which region it is over.
//!
//! WINDOWED-ONLY. Cap-gated on [`SutBlockInteract`] (the windowed gesture cap
//! no headless slice supplies), so `aggregate_transitions` structurally
//! excludes it from every headless draw — proven by a headless keystone run
//! with ZERO `WheelScroll` admissions.
//!
//! SOURCE-ROUTED occlusion axis (`over_footer`): a wheel is either over the
//! outer list (scrolls the main region) or over the sticky footer (occluded —
//! scrolls the footer body). Inc D emits the OUTER-LIST source; the sticky
//! footer source activates in Inc E when the reference models sticky footers.
//!
//! A wheel changes NO document/block state — it is pure viewport motion — so
//! `apply_to_ref` is a no-op (the reference models documents, not scroll
//! offsets). The signed δ rides on the transition instance; the windowed
//! harness reads it (with the before/after footer geometry) to feed the
//! `inv-wheel-two-mode-motion-law` / `inv-wheel-occlusion-routing` invariants.

use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;

/// The outer main-panel scroll region's bounds id (a real layout block).
const OUTER_LIST_ELEMENT: &str = "block:default-main-panel";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct WheelScroll {
    /// Occlusion source: true = over the sticky footer, false = over the outer
    /// list. Inc D emits only the outer-list source.
    pub over_footer: bool,
    /// The scroll region's bounds-registry id the wheel is dispatched over.
    pub element_id: String,
    /// Signed wheel δ in pixels (positive = scroll down).
    pub delta_y: i32,
}

impl<R> TransitionFactory<R> for WheelScroll
where
    R: RefLifecycle,
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        // Windowed-only (cap-gated) + needs a booted app. The outer-list source
        // is always available post-boot; the footer source (Inc E) requires the
        // reference to model a sticky footer.
        check(state.app_started(), Reason::AppNotStarted).map(|_| {
            let strat = prop::sample::select(vec![-120i32, -60, -30, 30, 60, 120])
                .prop_map(|delta_y| WheelScroll {
                    over_footer: false,
                    element_id: OUTER_LIST_ELEMENT.to_string(),
                    delta_y,
                })
                .boxed();
            // Low weight — a wheel is a frequent gesture but must not crowd out
            // block-modifying transitions.
            (2, strat)
        })
    }
}

impl<R> TransitionRef<R> for WheelScroll
where
    R: RefLifecycle,
{
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        check(state.app_started(), Reason::AppNotStarted)
    }

    fn apply_to_ref(&self, _state: &mut R) {
        // No-op: a wheel is pure viewport motion — no document/block state
        // changes. The δ lives on the transition instance.
    }
}

crate::cap_transition! {
    WheelScroll: SutBlockInteract,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        sut.scroll_over(&me.element_id, me.delta_y as f32).await;
    }
    sql_budget: |_me, _state| {
        // A wheel issues no SQL (pure viewport).
        ExpectedSql { reads: 0, writes: 0, ddl: 0, tolerance: 0 }
    }
}

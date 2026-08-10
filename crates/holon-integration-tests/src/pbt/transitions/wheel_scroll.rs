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

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, holon_macros::StepVocabulary)]
#[step_template("I scroll element {element_id} by {delta_y} over footer {over_footer}")]
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
            // Outer-list source (always available post-boot).
            let mut arms: Vec<(u32, BoxedStrategy<WheelScroll>)> = vec![(
                3,
                prop::sample::select(vec![-120i32, -60, -30, 30, 60, 120])
                    .prop_map(|delta_y| WheelScroll {
                        over_footer: false,
                        element_id: OUTER_LIST_ELEMENT.to_string(),
                        delta_y,
                    })
                    .boxed(),
            )];
            // Sticky-footer source — ACTIVE only when the reference models an
            // on-screen sticky footer (Journals-shaped stack). Source-routed:
            // the `over_footer` axis shrinks toward the outer-list arm (arm 0).
            if let Some(footer_id) = state.sticky_footer_element_id() {
                arms.push((
                    2,
                    prop::sample::select(vec![-120i32, -60, -30, 30, 60, 120])
                        .prop_map(move |delta_y| WheelScroll {
                            over_footer: true,
                            element_id: footer_id.clone(),
                            delta_y,
                        })
                        .boxed(),
                ));
            }
            let strat = proptest::strategy::Union::new_weighted(arms).boxed();
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
        // A wheel is pure viewport motion, but it still costs reads: measured
        // at 3 in all 5 retained samples (b=27..42, r=4..19), no variance.
        //
        // What those 3 statements ARE is NOT established — the retained corpus
        // has no per-statement breakdown for this transition. The only
        // focus_roots read on any prod path is a single un-filtered watch,
        // `SELECT region, root_id FROM focus_roots`
        // (`holon/src/sync/turso_block_query_source.rs:134`), observed
        // re-running redundantly elsewhere; 3 is NOT "one per region", and the
        // per-region `WHERE region = …` form exists only in test fixtures.
        //
        // So this is a measured ceiling, not a derivation. A breach means the
        // wheel path grew; re-measure it, never widen it to pass.
        ExpectedSql { reads: 3, writes: 0, ddl: 0, tolerance: 0 }
    }
}

#[cfg(test)]
mod shrink_tests {
    use proptest::prelude::*;
    use proptest::strategy::Strategy;
    use proptest::strategy::Union;
    use proptest::test_runner::TestError;
    use proptest::test_runner::TestRunner;

    use super::OUTER_LIST_ELEMENT;
    use super::WheelScroll;

    /// Reconstruct EXACTLY the source-routed strategy `weighted_generator`
    /// builds when BOTH sources are active (outer-list arm 0 + sticky-footer
    /// arm 1), so the shrink behaviour of the new arm is exercised directly.
    fn both_source_strat() -> proptest::strategy::BoxedStrategy<WheelScroll> {
        let deltas = || vec![-120i32, -60, -30, 30, 60, 120];
        Union::new_weighted(vec![
            (
                3u32,
                proptest::sample::select(deltas())
                    .prop_map(|delta_y| WheelScroll {
                        over_footer: false,
                        element_id: OUTER_LIST_ELEMENT.to_string(),
                        delta_y,
                    })
                    .boxed(),
            ),
            (
                2u32,
                proptest::sample::select(deltas())
                    .prop_map(|delta_y| WheelScroll {
                        over_footer: true,
                        element_id: "sticky-footer:x".to_string(),
                        delta_y,
                    })
                    .boxed(),
            ),
        ])
        .boxed()
    }

    /// Shrinking stays effective on the new arm: a forced (always-false)
    /// property drives the shrinker toward the canonical minimum — the
    /// outer-list source (arm 0) at the smallest delta index (`-120`). The
    /// `over_footer` occlusion axis shrinks toward `false` (arm 0).
    #[test]
    fn wheel_scroll_arm_shrinks_to_canonical_minimum() {
        let mut exercised = 0u32;
        for _ in 0..200 {
            let mut runner = TestRunner::default();
            match runner.run(&both_source_strat(), |_w| {
                // Forced failure: every drawn WheelScroll fails, so the runner
                // must MINIMISE it.
                prop_assert!(false, "forced");
                Ok(())
            }) {
                Err(TestError::Fail(_, minimal)) => {
                    exercised += 1;
                    assert!(
                        !minimal.over_footer && minimal.delta_y == -120,
                        "shrink did not reach the canonical minimum \
                         (over_footer=false, delta=-120): got {minimal:?}",
                    );
                }
                Ok(()) => panic!("forced-false property must fail"),
                Err(e) => panic!("unexpected: {e:?}"),
            }
        }
        assert!(exercised > 0, "no failing case generated — vacuous");
    }
}

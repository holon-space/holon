//! Transition: arrow-key navigation from the currently focused block.
//!
//! Mirrors the legacy logic split across `state_machine.rs:673-703`
//! (generator), `state_machine.rs:3181-3183` (precondition),
//! `state_machine.rs:2316-2451` (ref-state apply),
//! `sut.rs:2848-2897` (SUT apply), and
//! `transition_budgets.rs:199-205` (expected SQL).

use holon_api::Region;
use holon_frontend::navigation::NavDirection;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::CapRegion;
use holon_pbt_core::capabilities::RefArrowNav;
use holon_pbt_core::capabilities::RefLifecycle;
use holon_pbt_core::capabilities::RefViewSelection;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::check;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use proptest::strategy::Union;
use validated::Validated;

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::JOURNAL_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::NAV_DML_READS;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;

/// Navigate via arrow keys from the currently focused block in a region.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct ArrowNavigate {
    pub region: Region,
    pub direction: NavDirection,
    pub steps: u8,
}

impl<R: RefLifecycle + RefViewSelection + RefArrowNav<Direction = NavDirection>>
    TransitionFactory<R> for ArrowNavigate
{
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        Self::declared_caps()
    }

    type Reason = Reason;
    fn weighted_generator(state: &R) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let regions = [Region::Main, Region::LeftSidebar, Region::RightSidebar];
        let mut arms: Vec<(u32, BoxedStrategy<ArrowNavigate>)> = Vec::new();

        for region in &regions {
            if state.region_has_focus(*region) {
                // Determine available directions from navigator type. The render
                // name is main-panel/sidebar granular (CapRegion collapses the
                // two sidebars, which both list-navigate anyway).
                let cap_region = match region {
                    Region::Main => CapRegion::Main,
                    _ => CapRegion::Sidebar,
                };
                let render_name = state.active_render_expr_name(cap_region);
                let directions: Vec<NavDirection> = match render_name.as_deref() {
                    Some("tree") | Some("outline") => {
                        vec![
                            NavDirection::Up,
                            NavDirection::Down,
                            NavDirection::Left,
                            NavDirection::Right,
                        ]
                    }
                    _ => vec![NavDirection::Up, NavDirection::Down],
                };

                let r = region;
                let candidates: Vec<ArrowNavigate> = directions
                    .into_iter()
                    .flat_map(|direction| {
                        (1u8..=3u8).map(move |steps| ArrowNavigate {
                            region: *r,
                            direction,
                            steps,
                        })
                    })
                    .filter(|nav| nav.preconditions(state).is_good())
                    .collect();

                if !candidates.is_empty() {
                    arms.push((1, prop::sample::select(candidates).boxed()));
                }
            }
        }

        check(!arms.is_empty(), Reason::PreconditionFailed).map(|_| {
            let strat = Union::new_weighted(arms).boxed();
            (1, strat)
        })
    }
}

impl<R: RefLifecycle + RefArrowNav<Direction = NavDirection>> TransitionRef<R> for ArrowNavigate {
    type Reason = Reason;

    fn preconditions(&self, state: &R) -> Validated<(), Reason> {
        let checks: Vec<Validated<(), Reason>> = vec![
            check(state.app_started(), Reason::AppNotStarted),
            check(state.region_has_focus(self.region), Reason::MainFocusNotSet),
        ];
        checks
            .into_iter()
            .collect::<Validated<Vec<()>, _>>()
            .map(|_| ())
    }

    fn apply_to_ref(&self, state: &mut R) {
        // The whole cross-block cursor/focus walk (driving holon-frontend's
        // `CollectionNavigator`) lives in `RefArrowNav::apply_arrow_navigate`.
        state.apply_arrow_navigate(self.region, self.direction, self.steps);
    }
}

crate::cap_transition! {
    ArrowNavigate: ::holon_frontend::pbt_caps::SutArrowNavigate,
    where R: [ RefLifecycle ],
    |me, _state, sut| {
        // The cap's `region` is informational only (the driver emits global
        // arrow keystrokes); the left/right sidebar distinction collapses to
        // `Sidebar`.
        let region = match me.region {
            Region::Main => CapRegion::Main,
            Region::LeftSidebar | Region::RightSidebar => CapRegion::Sidebar,
        };
        sut.apply_arrow_navigate(region, me.direction, me.steps).await;
    }
    sql_budget: |me, state| {
        let steps = me.steps as usize;
        ExpectedSql {
            reads: REACTIVE_BASE + JOURNAL_READS + NAV_DML_READS + (steps * 2),
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + (steps * 2),
        }
    }
}

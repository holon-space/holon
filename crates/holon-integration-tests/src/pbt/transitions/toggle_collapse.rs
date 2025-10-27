//! Transition: toggle an `expand_toggle` widget's collapsed state.
//!
//! Delegates to shared `holon_pbt_core::ToggleCollapse`. Maps reasons.

use holon_api::EntityUri;
use holon_layout_testing::LayoutRef;
use holon_layout_testing::LayoutSut;
use holon_layout_testing::transitions::toggle_collapse::ToggleCollapseReason;
pub use holon_pbt_core::ToggleCollapse;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutBlockInteract;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::layout_bridge::SutClickAdapter;
use crate::pbt::reference_state::ReferenceState;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::ExpectedSql;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::REACTIVE_BASE;
#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::docs_tolerance;
use crate::pbt::validation::Reason;
use crate::pbt::validation::map_nevec;

fn map_reason(r: ToggleCollapseReason) -> Reason {
    match r {
        ToggleCollapseReason::NoCollapsibleTargets => Reason::NoCollapsibleTargets,
    }
}

fn parse_target(target_id: &str) -> EntityUri {
    EntityUri::parse(target_id)
        .unwrap_or_else(|e| panic!("[ToggleCollapse] invalid target_id {target_id:?}: {e}"))
}

impl TransitionFactory<ReferenceState> for ToggleCollapse {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutBlockInteract,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let ref_view = LayoutRef::new(state);
        match <ToggleCollapse as TransitionFactory<LayoutRef<'_, ReferenceState>>>::weighted_generator(&ref_view) {
            Validated::Good(x) => Validated::Good(x),
            Validated::Fail(reasons) => Validated::Fail(map_nevec(reasons, map_reason)),
        }
    }
}

impl TransitionRef<ReferenceState> for ToggleCollapse {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        if !state.action.app_started {
            return Validated::fail(Reason::AppNotStarted);
        }
        let ref_view = LayoutRef::new(state);
        match <ToggleCollapse as TransitionRef<LayoutRef<'_, ReferenceState>>>::preconditions(
            self, &ref_view,
        ) {
            Validated::Good(()) => Validated::Good(()),
            Validated::Fail(reasons) => Validated::Fail(map_nevec(reasons, map_reason)),
        }
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        // Mirror the click outcome locally — the SUT click path will
        // flip the gate's `Mutable<bool>` on the next render, and the
        // ref state must reflect that so subsequent preconditions stay
        // accurate.
        let uri = parse_target(&self.target_id);
        state.ui.tab.expanded_toggles.remove(&uri);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutBlockInteract> TransitionImpl<ReferenceState, S> for ToggleCollapse {
    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut S) {
        let ref_view = LayoutRef::new(state);
        let mut adapter = SutClickAdapter(sut);
        let mut layout_sut = LayoutSut::new(&mut adapter);
        <ToggleCollapse as TransitionImpl<
            LayoutRef<'_, ReferenceState>,
            LayoutSut<'_, SutClickAdapter<'_, S>>,
        >>::apply_to_sut(self, &ref_view, &mut layout_sut)
        .await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ToggleCollapse {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5,
        }
    }
}

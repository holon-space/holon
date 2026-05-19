//! Transition: toggle a drawer's open/closed state.
//!
//! Delegates to shared `holon_pbt_core::ToggleDrawer`. Maps reasons,
//! mirrors the drawer-open flip into `ReferenceState.drawer_open`.

use holon_layout_testing::transitions::toggle_drawer::ToggleDrawerReason;
use holon_layout_testing::{LayoutRef, LayoutRefState, LayoutSut};
use holon_pbt_core::{TransitionFactory, TransitionImpl, TransitionRef};
use proptest::strategy::BoxedStrategy;
use validated::Validated;

pub use holon_pbt_core::ToggleDrawer;

use crate::pbt::layout_bridge::SutClickAdapter;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transition_dispatch::SutHandle;
use crate::pbt::validation::{Reason, map_nevec};

#[cfg(feature = "otel-testing")]
use crate::pbt::transition_budgets::{ExpectedSql, REACTIVE_BASE, docs_tolerance};

fn map_reason(r: ToggleDrawerReason) -> Reason {
    match r {
        ToggleDrawerReason::NoDrawerHandles => Reason::NoDrawerHandles,
    }
}

impl TransitionFactory<ReferenceState> for ToggleDrawer {
    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let ref_view = LayoutRef::new(state);
        match <ToggleDrawer as TransitionFactory<LayoutRef<'_, ReferenceState>>>::weighted_generator(
            &ref_view,
        ) {
            Validated::Good(x) => Validated::Good(x),
            Validated::Fail(reasons) => Validated::Fail(map_nevec(reasons, map_reason)),
        }
    }
}

impl TransitionRef<ReferenceState> for ToggleDrawer {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        if !state.app_started {
            return Validated::fail(Reason::AppNotStarted);
        }
        let ref_view = LayoutRef::new(state);
        match <ToggleDrawer as TransitionRef<LayoutRef<'_, ReferenceState>>>::preconditions(
            self, &ref_view,
        ) {
            Validated::Good(()) => Validated::Good(()),
            Validated::Fail(reasons) => Validated::Fail(map_nevec(reasons, map_reason)),
        }
    }

    fn apply_to_ref(&self, state: &mut ReferenceState) {
        // Flip the recorded open/closed bit; default-true so unknown
        // ids start open.
        let current = state.drawer_is_open(&self.block_id);
        state.drawer_open.insert(self.block_id.clone(), !current);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutHandle> TransitionImpl<ReferenceState, S> for ToggleDrawer {
    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut S) {
        let ref_view = LayoutRef::new(state);
        let mut adapter = SutClickAdapter(sut);
        let mut layout_sut = LayoutSut::new(&mut adapter);
        <ToggleDrawer as TransitionImpl<
            LayoutRef<'_, ReferenceState>,
            LayoutSut<'_, SutClickAdapter<'_, S>>,
        >>::apply_to_sut(self, &ref_view, &mut layout_sut)
        .await;
    }
}

#[cfg(feature = "otel-testing")]
impl crate::pbt::transition_budgets::SqlBudget for ToggleDrawer {
    fn expected_sql(&self, state: &ReferenceState) -> ExpectedSql {
        ExpectedSql {
            reads: REACTIVE_BASE + 10,
            writes: 0,
            ddl: 0,
            tolerance: docs_tolerance(state) + 5,
        }
    }
}

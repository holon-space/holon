//! Transition: switch the active view mode on a rendered ViewModeSwitcher.
//!
//! @pbt rung input-pipeline
//!   RESIDUAL (concrete ReferenceState, audit TR-RESID): drives SutBlockInteract
//!   click via the LayoutSut/SutClickAdapter bridge. Blocked from generic-R by
//!   the concrete `LayoutRef::new(&ReferenceState)` layout-testing bridge.
//! @pbt covers view-mode-switch — ViewModeSwitcher click -> mode change
//!
//! Delegates entirely to the shared `holon_pbt_core::SwitchViewMode`
//! variant + the `TransitionFactory` / `TransitionImpl` bodies in
//! `holon_layout_testing::transitions::switch_view_mode`. This file
//! exists only to satisfy the file-per-variant arch rule and to map the
//! shared `SwitchViewModeReason` into the integration-tests `Reason`
//! enum (the enum drives the rejection histogram).

use holon_layout_testing::LayoutRef;
use holon_layout_testing::LayoutSut;
use holon_layout_testing::transitions::switch_view_mode::SwitchViewModeReason;
pub use holon_pbt_core::SwitchViewMode;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::map_nevec;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::layout_bridge::SutClickAdapter;
use crate::pbt::reference_state::ReferenceState;

fn map_reason(r: SwitchViewModeReason) -> Reason {
    match r {
        SwitchViewModeReason::NoSwitchableHandles => Reason::NoSwitchableHandles,
    }
}

impl TransitionFactory<ReferenceState> for SwitchViewMode {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutBlockInteract,
        >()]
    }

    type Reason = Reason;
    fn weighted_generator(state: &ReferenceState) -> Validated<(u32, BoxedStrategy<Self>), Reason> {
        let ref_view = LayoutRef::new(state);
        match <SwitchViewMode as TransitionFactory<LayoutRef<'_, ReferenceState>>>::weighted_generator(&ref_view) {
            Validated::Good(x) => Validated::Good(x),
            Validated::Fail(reasons) => Validated::Fail(map_nevec(reasons, map_reason)),
        }
    }
}

impl TransitionRef<ReferenceState> for SwitchViewMode {
    type Reason = Reason;

    fn preconditions(&self, state: &ReferenceState) -> Validated<(), Reason> {
        if !state.action.app_started {
            return Validated::fail(Reason::AppNotStarted);
        }
        let ref_view = LayoutRef::new(state);
        match <SwitchViewMode as TransitionRef<LayoutRef<'_, ReferenceState>>>::preconditions(
            self, &ref_view,
        ) {
            Validated::Good(()) => Validated::Good(()),
            Validated::Fail(reasons) => Validated::Fail(map_nevec(reasons, map_reason)),
        }
    }

    fn apply_to_ref(&self, _: &mut ReferenceState) {
        // Shared variant has no ref-state model for VMS state today.
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutBlockInteract> TransitionImpl<ReferenceState, S> for SwitchViewMode {
    async fn apply_to_sut(&self, state: &ReferenceState, sut: &mut S) {
        let ref_view = LayoutRef::new(state);
        let mut adapter = SutClickAdapter(sut);
        let mut layout_sut = LayoutSut::new(&mut adapter);
        <SwitchViewMode as TransitionImpl<
            LayoutRef<'_, ReferenceState>,
            LayoutSut<'_, SutClickAdapter<'_, S>>,
        >>::apply_to_sut(self, &ref_view, &mut layout_sut)
        .await;
    }
}

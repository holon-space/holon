//! Transition: toggle a drawer's open/closed state.
//!
//! Delegates to shared `holon_pbt_core::ToggleDrawer`. Maps reasons,
//! mirrors the drawer-open flip into `ReferenceState.drawer_open`.

use holon_layout_testing::LayoutRef;
use holon_layout_testing::LayoutSut;
use holon_layout_testing::transitions::toggle_drawer::ToggleDrawerReason;
pub use holon_pbt_core::ToggleDrawer;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use holon_pbt_core::capabilities::RefToggleMut;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::map_nevec;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::layout_bridge::SutClickAdapter;
use crate::pbt::reference_state::ReferenceState;

fn map_reason(r: ToggleDrawerReason) -> Reason {
    match r {
        ToggleDrawerReason::NoDrawerHandles => Reason::NoDrawerHandles,
    }
}

impl TransitionFactory<ReferenceState> for ToggleDrawer {
    fn required_caps() -> Vec<::holon_pbt_core::composition::CapId> {
        vec![::holon_pbt_core::composition::CapId::of::<
            dyn ::holon_pbt_core::capabilities::SutBlockInteract,
        >()]
    }

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
        if !state.action.app_started {
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
        // Flip the recorded open/closed bit (default-open) — single-sourced in
        // `RefToggleMut::toggle_drawer`.
        state.toggle_drawer(&self.block_id);
    }
}

#[allow(async_fn_in_trait)]
impl<S: SutBlockInteract> TransitionImpl<ReferenceState, S> for ToggleDrawer {
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

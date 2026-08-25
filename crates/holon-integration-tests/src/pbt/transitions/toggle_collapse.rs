//! Transition: toggle an `expand_toggle` widget's collapsed state.
//!
//! @pbt rung input-pipeline
//!   RESIDUAL (concrete ReferenceState, audit TR-RESID): SutBlockInteract click
//!   via LayoutSut bridge. Blocked by the concrete LayoutRef bridge.
//! @pbt covers collapse-click — expand_toggle click -> collapse
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
use holon_pbt_core::capabilities::RefToggleMut;
use holon_pbt_core::capabilities::SutBlockInteract;
use holon_pbt_core::validation::Reason;
use holon_pbt_core::validation::map_nevec;
use proptest::strategy::BoxedStrategy;
use validated::Validated;

use crate::pbt::layout_bridge::SutClickAdapter;
use crate::pbt::reference_state::ReferenceState;

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
        // A caret click is a TOGGLE: the SUT flips whatever the rendered
        // gate holds, and that gate is seeded from the block's `collapsed`
        // document field. Mirror the flip off the ref block's `collapsed`
        // so both directions of the gesture stay modelled (the generator
        // only picks expanded targets, so generated sequences still
        // collapse; replayed scenarios may re-expand).
        let uri = parse_target(&self.target_id);
        let now_collapsed = state
            .domain
            .block_state
            .blocks
            .get(&uri)
            .unwrap_or_else(|| {
                panic!(
                    "[ToggleCollapse::apply_to_ref] {uri} is not in the reference block state — \
                     a caret cannot be clicked on a block the model does not know about"
                )
            })
            .collapsed;
        state.set_expanded(&uri, now_collapsed);
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

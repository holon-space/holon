//! Shared `SwitchViewMode` semantics: pick a (block, mode) pair from
//! the ref-state's switchable handles; clicking the canonical VMS
//! button id flips the VMS to the requested mode through whatever
//! click pipeline the frontend's `Clickable` impl runs.

use holon_frontend::vms_button_id_for;
use holon_pbt_core::SwitchViewMode;
use holon_pbt_core::TransitionFactory;
use holon_pbt_core::TransitionImpl;
use holon_pbt_core::TransitionRef;
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated::Good;
use validated::Validated::{self};

use crate::sut::Clickable;
use crate::sut::LayoutRef;
use crate::sut::LayoutRefState;
use crate::sut::LayoutSut;

/// Why a `SwitchViewMode` generator might decline a given state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SwitchViewModeReason {
    NoSwitchableHandles,
    NoModeSwitchableSurface,
    ModeNotOfferedByBlock,
}

impl<R> TransitionFactory<LayoutRef<'_, R>> for SwitchViewMode
where
    R: LayoutRefState + ?Sized,
{
    type Reason = SwitchViewModeReason;
    fn weighted_generator(
        state: &LayoutRef<'_, R>,
    ) -> Validated<(u32, BoxedStrategy<Self>), Self::Reason> {
        let handles: std::sync::Arc<Vec<(String, Vec<String>)>> = std::sync::Arc::new(
            state
                .switchable_handles()
                .iter()
                .filter(|h| h.mode_names.len() >= 2)
                .map(|h| (h.block_id.clone(), h.mode_names.clone()))
                .collect(),
        );
        if handles.is_empty() {
            return Validated::fail(SwitchViewModeReason::NoSwitchableHandles);
        }
        let len = handles.len();
        let handles_for_flat = handles.clone();
        let strat = (0..len)
            .prop_flat_map(move |i| {
                let handles = handles_for_flat.clone();
                let num_modes = handles[i].1.len();
                (Just(i), 0..num_modes).prop_map(move |(i, m)| SwitchViewMode {
                    block_id: handles[i].0.clone(),
                    target_mode: handles[i].1[m].clone(),
                })
            })
            .boxed();
        Good((1, strat))
    }
}

impl<R> TransitionRef<LayoutRef<'_, R>> for SwitchViewMode
where
    R: LayoutRefState + ?Sized,
{
    type Reason = SwitchViewModeReason;
    /// The VMS button `apply_to_sut` clicks only exists if the ref-state
    /// surfaces a switchable handle for this block offering this mode. Without
    /// one the click resolves to an entity nothing answers and degrades to bare
    /// focus, so a fixture asking for the switch would pass having changed
    /// nothing — refuse it here instead.
    fn preconditions(&self, state: &LayoutRef<'_, R>) -> Validated<(), Self::Reason> {
        let Some(handle) = state
            .switchable_handles()
            .iter()
            .find(|h| h.block_id == self.block_id)
        else {
            return Validated::fail(SwitchViewModeReason::NoModeSwitchableSurface);
        };
        if !handle.mode_names.contains(&self.target_mode) {
            return Validated::fail(SwitchViewModeReason::ModeNotOfferedByBlock);
        }
        Good(())
    }
    fn apply_to_ref(&self, _: &mut LayoutRef<'_, R>) {}
}

impl<R, S> TransitionImpl<LayoutRef<'_, R>, LayoutSut<'_, S>> for SwitchViewMode
where
    R: LayoutRefState + ?Sized,
    S: Clickable + ?Sized,
{
    async fn apply_to_sut(&self, _: &LayoutRef<'_, R>, sut: &mut LayoutSut<'_, S>) {
        let button_id = vms_button_id_for(&self.block_id, &self.target_mode);
        sut.click_at_element(&button_id);
    }
}

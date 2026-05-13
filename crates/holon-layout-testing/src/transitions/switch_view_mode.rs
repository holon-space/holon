//! Shared `SwitchViewMode` semantics: pick a (block, mode) pair from
//! the ref-state's switchable handles; clicking the canonical VMS
//! button id flips the VMS to the requested mode through whatever
//! click pipeline the frontend's `Clickable` impl runs.

use holon_frontend::vms_button_id_for;
use holon_pbt_core::{SwitchViewMode, TransitionFactory, TransitionImpl};
use proptest::prelude::*;
use proptest::strategy::BoxedStrategy;
use validated::Validated::{self, Good};

use crate::sut::{Clickable, LayoutRef, LayoutRefState, LayoutSut};

/// Why a `SwitchViewMode` generator might decline a given state.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SwitchViewModeReason {
    NoSwitchableHandles,
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

impl<R, S> TransitionImpl<LayoutRef<'_, R>, LayoutSut<'_, S>> for SwitchViewMode
where
    R: LayoutRefState + ?Sized,
    S: Clickable + ?Sized,
{
    type Reason = SwitchViewModeReason;
    fn preconditions(&self, _: &LayoutRef<'_, R>) -> Validated<(), Self::Reason> {
        Good(())
    }
    fn apply_to_ref(&self, _: &mut LayoutRef<'_, R>) {}
    async fn apply_to_sut(&self, _: &LayoutRef<'_, R>, sut: &mut LayoutSut<'_, S>) {
        let button_id = vms_button_id_for(&self.block_id, &self.target_mode);
        sut.click_at_element(&button_id);
    }
}

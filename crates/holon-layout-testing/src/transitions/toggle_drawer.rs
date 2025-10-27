//! Shared `ToggleDrawer` semantics: pick a drawer from the ref-state's
//! drawer handles; click the canonical drawer-toggle widget at
//! `drawer_toggle_id_for(block_id)`. Routed through the same production
//! `on_mouse_down → set_widget_open` chain a real tap traverses.

use holon_frontend::drawer_toggle_id_for;
use holon_pbt_core::ToggleDrawer;
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

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToggleDrawerReason {
    NoDrawerHandles,
}

impl<R> TransitionFactory<LayoutRef<'_, R>> for ToggleDrawer
where
    R: LayoutRefState + ?Sized,
{
    type Reason = ToggleDrawerReason;
    fn weighted_generator(
        state: &LayoutRef<'_, R>,
    ) -> Validated<(u32, BoxedStrategy<Self>), Self::Reason> {
        let ids: Vec<String> = state
            .drawer_handles()
            .iter()
            .map(|d| d.block_id.clone())
            .collect();
        if ids.is_empty() {
            return Validated::fail(ToggleDrawerReason::NoDrawerHandles);
        }
        let strat = proptest::sample::select(ids)
            .prop_map(|block_id| ToggleDrawer { block_id })
            .boxed();
        Good((1, strat))
    }
}

impl<R> TransitionRef<LayoutRef<'_, R>> for ToggleDrawer
where
    R: LayoutRefState + ?Sized,
{
    type Reason = ToggleDrawerReason;
    fn preconditions(&self, _: &LayoutRef<'_, R>) -> Validated<(), Self::Reason> {
        Good(())
    }
    fn apply_to_ref(&self, _: &mut LayoutRef<'_, R>) {}
}

impl<R, S> TransitionImpl<LayoutRef<'_, R>, LayoutSut<'_, S>> for ToggleDrawer
where
    R: LayoutRefState + ?Sized,
    S: Clickable + ?Sized,
{
    async fn apply_to_sut(&self, _: &LayoutRef<'_, R>, sut: &mut LayoutSut<'_, S>) {
        let toggle_id = drawer_toggle_id_for(&self.block_id);
        sut.click_at_element(&toggle_id);
    }
}

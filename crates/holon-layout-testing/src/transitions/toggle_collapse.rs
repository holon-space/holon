//! Shared `ToggleCollapse` semantics: pick an expand-toggle target
//! from the ref-state; click the canonical
//! `expand_toggle_id_for(target_id)` widget.

use holon_frontend::geometry::expand_toggle_id_for;
use holon_pbt_core::ToggleCollapse;
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
pub enum ToggleCollapseReason {
    NoCollapsibleTargets,
}

impl<R> TransitionFactory<LayoutRef<'_, R>> for ToggleCollapse
where
    R: LayoutRefState + ?Sized,
{
    type Reason = ToggleCollapseReason;
    fn weighted_generator(
        state: &LayoutRef<'_, R>,
    ) -> Validated<(u32, BoxedStrategy<Self>), Self::Reason> {
        let ids = state.collapsible_target_ids();
        if ids.is_empty() {
            return Validated::fail(ToggleCollapseReason::NoCollapsibleTargets);
        }
        let strat = proptest::sample::select(ids)
            .prop_map(|target_id| ToggleCollapse { target_id })
            .boxed();
        Good((1, strat))
    }
}

impl<R> TransitionRef<LayoutRef<'_, R>> for ToggleCollapse
where
    R: LayoutRefState + ?Sized,
{
    type Reason = ToggleCollapseReason;
    fn preconditions(&self, _: &LayoutRef<'_, R>) -> Validated<(), Self::Reason> {
        Good(())
    }
    fn apply_to_ref(&self, _: &mut LayoutRef<'_, R>) {}
}

impl<R, S> TransitionImpl<LayoutRef<'_, R>, LayoutSut<'_, S>> for ToggleCollapse
where
    R: LayoutRefState + ?Sized,
    S: Clickable + ?Sized,
{
    async fn apply_to_sut(&self, _: &LayoutRef<'_, R>, sut: &mut LayoutSut<'_, S>) {
        let toggle_id = expand_toggle_id_for(&self.target_id);
        sut.click_at_element(&toggle_id);
    }
}

//! Aggregator enum for user-visible UI state changes shared across PBTs.
//!
//! The variant *structs* (`SwitchViewMode`, `ToggleDrawer`,
//! `DeliverBlockContent`) live in `holon-pbt-core` so multiple PBTs can
//! share them. The behaviour (how to actually apply each variant to a
//! running frontend) lives in the consumer crate's
//! `support/transitions/<variant>.rs` file as
//! `impl TransitionImpl<(), <ConcreteSut>> for <Variant>`.

use holon_pbt_core::DeliverBlockContent;
use holon_pbt_core::SwitchViewMode;
use holon_pbt_core::ToggleCollapse;
use holon_pbt_core::ToggleDrawer;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UiInteraction {
    SwitchViewMode(SwitchViewMode),
    ToggleDrawer(ToggleDrawer),
    ToggleCollapse(ToggleCollapse),
    DeliverBlockContent(DeliverBlockContent),
}

impl From<SwitchViewMode> for UiInteraction {
    fn from(t: SwitchViewMode) -> Self {
        Self::SwitchViewMode(t)
    }
}

impl From<ToggleDrawer> for UiInteraction {
    fn from(t: ToggleDrawer) -> Self {
        Self::ToggleDrawer(t)
    }
}

impl From<ToggleCollapse> for UiInteraction {
    fn from(t: ToggleCollapse) -> Self {
        Self::ToggleCollapse(t)
    }
}

impl From<DeliverBlockContent> for UiInteraction {
    fn from(t: DeliverBlockContent) -> Self {
        Self::DeliverBlockContent(t)
    }
}

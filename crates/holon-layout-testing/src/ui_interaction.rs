//! Aggregator enum for UI interactions used by scenario generators and
//! the scenario runner.
//!
//! The variant *structs* (`SwitchViewMode`, `ToggleDrawer`,
//! `DeliverBlockContent`) live in `holon-pbt-core` so multiple PBTs can
//! share them. The behaviour (how to actually apply each variant to a
//! running frontend) lives in the consumer crate's
//! `support/transitions/<variant>.rs` file as
//! `impl TransitionImpl<(), <ConcreteSut>> for <Variant>`.

use holon_pbt_core::{DeliverBlockContent, SwitchViewMode, ToggleCollapse, ToggleDrawer};

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

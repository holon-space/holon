//! Shared UI interaction events for proptest-based UI testing.
//!
//! Moved here from `crates/holon-integration-tests/src/pbt/ui_interaction.rs`
//! so `holon-layout-testing` can own the type without depending on the
//! heavier `holon-integration-tests` crate.

/// A single piece of user-visible UI state change.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum UiInteraction {
    /// Switch a mode-switchable block to the named mode.
    ///
    /// The test harness routes this through `UserDriver::click_element`
    /// using `vms_button_id_for(block_id, target_mode)` to locate the
    /// rendered VMS button in the bounds registry.
    SwitchViewMode {
        /// The VMS's `entity_uri` as rendered (`EntityUri::to_string()`).
        block_id: String,
        /// The target mode name (e.g. `"table_view"`, `"tree_view"`).
        target_mode: String,
    },
    /// Toggle a drawer's open/closed state.
    ToggleDrawer {
        /// The drawer's block_id (e.g. `"block:default-left-sidebar"`).
        block_id: String,
    },
    /// Deliver deferred live_block content. Simulates async data arrival:
    /// the block was registered with an empty placeholder, and this action
    /// pushes the real content through the structural_changes stream.
    /// No UI click needed — this is a programmatic data delivery.
    DeliverBlockContent { block_id: String },
}

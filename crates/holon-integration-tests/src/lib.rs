//! Shared test infrastructure for Holon integration tests
//!
//! This module provides reusable components for both property-based tests
//! and Cucumber BDD tests.

pub mod assertions;
pub mod debug_pause;
/// `display_assertions` moved to `holon-layout-testing`. Re-exported here
/// so call sites inside this crate can keep using `crate::display_assertions::*`.
/// Only available with the `pbt` feature because that's the only feature
/// that pulls `holon-layout-testing` into the dep tree.
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions;
pub mod mutation_driver;
pub mod org_utils;
#[cfg(feature = "pbt")]
pub mod pbt;
pub mod pbt_mcp_fake;
pub mod polling;
pub mod screenshot_overlay;
pub mod test_environment;
#[cfg(feature = "otel-testing")]
pub mod test_tracing;
pub mod ui_driver;
pub mod widget_state;

pub use assertions::{assert_block_order, assert_blocks_equivalent, normalize_block};
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions::{
    DiffableTree, OrderedSubsetResult, TreeDiff, assert_display_trees_match, is_ordered_subset,
    tree_diff,
};
pub use mutation_driver::{DirectUserDriver, ReactiveEngineDriver, UserDriver};
pub use org_utils::{
    INTERNAL_PROPS, assign_reference_sequences, assign_reference_sequences_canonical,
    extract_first_block_id, serialize_block_recursive, serialize_blocks_to_org,
    serialize_blocks_to_org_with_doc,
};
pub use polling::{
    drain_stream, wait_for_block, wait_for_block_count, wait_for_file_condition,
    wait_for_text_in_widget, wait_until,
};
pub use screenshot_overlay::{DEFAULT_OVERLAY_ALPHA, Overlay, Phase, Verdict};
pub use test_environment::{
    LoroCorruptionType, TestContext, TestContextBuilder, TestEnvironment, TestEnvironmentBuilder,
};
pub use ui_driver::{
    CapturedScreenshot, FfiDriver, GeometryDriver, ScreenshotBackend, SignalScreenshotWatcher,
    UiDriver, XcapBackend,
};
pub use widget_state::{WidgetLocator, WidgetStateModel, apply_cdc_event_to_vec};

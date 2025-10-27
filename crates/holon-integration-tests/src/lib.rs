//! Shared test infrastructure for Holon integration tests
//!
//! This module provides reusable components for both property-based tests
//! and Cucumber BDD tests.

pub mod assertions;
pub mod display_assertions;
pub mod mutation_driver;
pub mod org_utils;
#[cfg(feature = "pbt")]
pub mod pbt;
pub mod polling;
pub mod test_environment;
pub mod ui_driver;
pub mod widget_state;

pub use assertions::{
    assert_block_order, assert_blocks_equivalent, block_belongs_to_document,
    find_document_for_block, normalize_block,
};
pub use display_assertions::{
    OrderedSubsetResult, TreeDiff, assert_display_trees_match, is_ordered_subset, tree_diff,
};
pub use mutation_driver::{DirectMutationDriver, MutationDriver};
pub use org_utils::{
    INTERNAL_PROPS, assign_reference_sequences, assign_reference_sequences_canonical,
    extract_first_block_id, serialize_block_recursive, serialize_blocks_to_org,
};
pub use polling::{
    drain_stream, wait_for_block, wait_for_block_count, wait_for_file_condition,
    wait_for_text_in_widget, wait_until,
};
pub use test_environment::{
    LoroCorruptionType, TestContext, TestContextBuilder, TestEnvironment, TestEnvironmentBuilder,
    TestExtras,
};
pub use ui_driver::{
    CapturedScreenshot, FfiDriver, GeometryDriver, ScreenshotBackend, UiDriver, XcapBackend,
};
pub use widget_state::{WidgetLocator, WidgetStateModel, apply_cdc_event_to_vec};

//! @c4 component
//! @c4 layer Testing
//! Pattern: Test Harness
//! @c4 uses holon "core orchestration" "Rust"
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-app "DI composition root" "Rust"
//! @c4 uses holon-block-roundtrip-testing "block round-trip generators" "Rust"
//! @c4 uses holon-filesystem "filesystem ports" "Rust"
//! @c4 uses holon-frontend "frontend session abstraction" "Rust"
//! @c4 uses holon-layout-testing "layout PBT primitives" "Rust"
//! @c4 uses holon-mcp "MCP frontend" "Rust"
//! @c4 uses holon-mcp-client "MCP client providers" "Rust"
//! @c4 uses holon-orgmode "org-mode sync I/O" "Rust"
//! @c4 uses holon-pbt-core "PBT transition traits" "Rust"
//! @c4 uses holon-loro "Loro CRDT backend & P2P sync" "Rust"
//! @c4 uses holon-profiles "entity profile resolution" "Rust"
//!
//! Shared test infrastructure for Holon integration tests
//!
//! This module provides reusable components for both property-based tests
//! and Cucumber BDD tests.

pub mod assertions;
pub mod debug_pause;

/// `cap_transition!` now lives in `holon-pbt-core` (shared with the
/// per-subsystem companion PBT crates). Re-exported at this crate root so the
/// many `crate::cap_transition!` call sites in `pbt::transitions::*` keep
/// resolving.
pub use holon_pbt_core::cap_transition;
pub mod fake_mcp_module;
/// `display_assertions` moved to `holon-layout-testing`. Re-exported here
/// so call sites inside this crate can keep using
/// `crate::display_assertions::*`. Only available with the `pbt` feature
/// because that's the only feature that pulls `holon-layout-testing` into the
/// dep tree.
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions;
#[cfg(feature = "pbt")]
pub mod mcp_user_driver;
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

pub use assertions::assert_block_order;
pub use assertions::assert_blocks_equivalent;
pub use assertions::normalize_block;
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions::DiffableTree;
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions::OrderedSubsetResult;
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions::TreeDiff;
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions::assert_display_trees_match;
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions::is_ordered_subset;
#[cfg(feature = "pbt")]
pub use holon_layout_testing::display_assertions::tree_diff;
#[cfg(feature = "pbt")]
pub use mcp_user_driver::DEFAULT_MCP_PORT;
#[cfg(feature = "pbt")]
pub use mcp_user_driver::McpUserDriver;
#[cfg(feature = "pbt")]
pub use mcp_user_driver::mcp_base_url_from_env;
pub use mutation_driver::DirectUserDriver;
pub use mutation_driver::ReactiveEngineDriver;
pub use mutation_driver::UserDriver;
pub use org_utils::INTERNAL_PROPS;
pub use org_utils::assign_reference_sequences;
pub use org_utils::assign_reference_sequences_canonical;
pub use org_utils::extract_first_block_id;
pub use org_utils::serialize_block_recursive;
pub use org_utils::serialize_blocks_to_org;
pub use org_utils::serialize_blocks_to_org_with_doc;
pub use polling::drain_stream;
pub use polling::wait_for_block;
pub use polling::wait_for_block_count;
pub use polling::wait_for_file_condition;
pub use polling::wait_for_text_in_widget;
pub use polling::wait_until;
pub use screenshot_overlay::DEFAULT_OVERLAY_ALPHA;
pub use screenshot_overlay::Overlay;
pub use screenshot_overlay::Phase;
pub use screenshot_overlay::Verdict;
pub use test_environment::TestContext;
pub use test_environment::TestContextBuilder;
pub use test_environment::TestEnvironment;
pub use test_environment::TestEnvironmentBuilder;
pub use ui_driver::CapturedScreenshot;
pub use ui_driver::FfiDriver;
pub use ui_driver::GeometryDriver;
pub use ui_driver::ScreenshotBackend;
pub use ui_driver::SignalScreenshotWatcher;
pub use ui_driver::UiDriver;
pub use ui_driver::XcapBackend;
pub use widget_state::WidgetLocator;
pub use widget_state::WidgetStateModel;
pub use widget_state::apply_cdc_event_to_vec;

//! Re-export `UiInteraction` from `holon-layout-testing` where it now lives.
//!
//! Moved to `holon-layout-testing` so the shared layout-testing crate can
//! own the type without a dependency on the heavier `holon-integration-tests`.
pub use holon_layout_testing::UiInteraction;

//! @c4 component
//! @c4 layer Testing
//! Pattern: Test Harness
//! @c4 uses holon-api "shared value & operation types" "Rust"
//! @c4 uses holon-frontend "frontend session abstraction" "Rust"
//! @c4 uses holon-pbt-core "PBT transition traits" "Rust"
//!
//! Shared layout-testing primitives for Holon's property-based UI tests.
//!
//! This crate contains everything needed to write layout property tests
//! against any frontend, without any GPUI dependency:
//!
//! - `BoundsSnapshot` / `Rect`: flat geometry types populated by a frontend
//!   render pass and read by the invariant functions.
//! - Layout invariants (`assert_nonempty`, `assert_all_nonzero`, etc.):
//!   frontend-agnostic checks that a render produced sane geometry.
//! - `BlockTreeRegistry`: instance-owned registry mapping block IDs to reactive
//!   mode thunks. Replaces the old `static OnceLock<Mutex<…>>` in
//!   `frontends/gpui/tests/support/mod.rs`.
//! - `vms_button_id_for`: canonical element-id scheme shared between the VMS
//!   builder (which tags each mode button) and the test harness (which uses the
//!   id to locate the button in `BoundsRegistry`).
//! - `UiInteraction`: the shared vocabulary of user-visible UI state changes.
//! - `Shape`, `Blueprint`, `BlockHandle`: thunk-based handle types for
//!   describing `ReactiveViewModel` trees.

pub mod blueprint;
pub mod display_assertions;
pub mod invariants;
pub mod live_tree;
pub mod registry;
pub mod snapshot;
pub mod sut;
pub mod transitions;
pub mod ui_interaction;
pub mod vms;

pub use blueprint::BlockHandle;
pub use blueprint::Blueprint;
pub use blueprint::Shape;
pub use holon_pbt_core::DeliverBlockContent;
pub use holon_pbt_core::SwitchViewMode;
pub use holon_pbt_core::ToggleCollapse;
pub use holon_pbt_core::ToggleDrawer;
pub use invariants::assert_all_nonzero;
pub use invariants::assert_all_nonzero_except;
pub use invariants::assert_containment;
pub use invariants::assert_content_fidelity;
pub use invariants::assert_layout_ok;
pub use invariants::assert_no_sibling_overlap;
pub use invariants::assert_nonempty;
pub use registry::BlockEntry;
pub use registry::BlockTreeRegistry;
pub use registry::BlockTreeThunk;
pub use snapshot::BoundsSnapshot;
pub use snapshot::VISIBLE_LEAF_TYPES;
pub use sut::Clickable;
pub use sut::LayoutRef;
pub use sut::LayoutRefState;
pub use sut::LayoutSut;
pub use sut::LiveBlockSink;
pub use ui_interaction::UiInteraction;
pub use vms::drawer_toggle_id_for;
pub use vms::vms_button_id_for;

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
//! - `BlockTreeRegistry`: instance-owned registry mapping block IDs to
//!   reactive mode thunks. Replaces the old `static OnceLock<Mutex<…>>` in
//!   `frontends/gpui/tests/support/mod.rs`.
//! - `vms_button_id_for`: canonical element-id scheme shared between the
//!   VMS builder (which tags each mode button) and the test harness (which
//!   uses the id to locate the button in `BoundsRegistry`).
//! - `UiInteraction`: the shared vocabulary of user-visible UI state changes.
//! - `Shape`, `Blueprint`, `BlockHandle`, `Scenario`: proptest strategy types
//!   for generating random `ReactiveViewModel` trees and action sequences.
//! - `run_scenario`: the closure-driven scenario runner.

pub mod blueprint;
pub mod display_assertions;
pub mod generators;
pub mod invariants;
pub mod live_tree;
pub mod registry;
pub mod scenario;
pub mod scene_state;
pub mod snapshot;
pub mod sut;
pub mod transitions;
pub mod ui_interaction;
pub mod vms;

pub use blueprint::{BlockHandle, Blueprint, Shape};
pub use holon_pbt_core::{DeliverBlockContent, SwitchViewMode, ToggleCollapse, ToggleDrawer};
pub use invariants::{
    assert_all_nonzero, assert_all_nonzero_except, assert_containment, assert_content_fidelity,
    assert_layout_ok, assert_no_sibling_overlap, assert_nonempty,
};
pub use registry::{BlockEntry, BlockTreeRegistry, BlockTreeThunk};
pub use scenario::{run_scenario, Scenario, StepInput};
pub use scene_state::SceneState;
pub use snapshot::{BoundsSnapshot, VISIBLE_LEAF_TYPES};
pub use sut::{Clickable, LayoutRef, LayoutRefState, LayoutSut, LiveBlockSink};
pub use ui_interaction::UiInteraction;
pub use vms::{drawer_toggle_id_for, vms_button_id_for};

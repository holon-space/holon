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
pub mod snapshot;
pub mod ui_interaction;
pub mod vms;

pub use blueprint::{BlockHandle, Blueprint, Shape};
pub use invariants::{
    assert_all_nonzero, assert_all_nonzero_except, assert_containment, assert_content_fidelity,
    assert_layout_ok, assert_no_sibling_overlap, assert_nonempty,
};
pub use registry::{BlockEntry, BlockTreeRegistry, BlockTreeThunk};
pub use scenario::{compute_final_drawer_states, run_scenario, Scenario, StepInput};
pub use snapshot::{BoundsSnapshot, VISIBLE_LEAF_TYPES};
pub use ui_interaction::UiInteraction;
pub use vms::vms_button_id_for;

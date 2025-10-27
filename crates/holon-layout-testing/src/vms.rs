//! Canonical element-ID scheme for View Mode Switcher (VMS) mode buttons.
//!
//! The canonical definition lives in `holon_frontend::geometry::vms_button_id_for`
//! so production builder crates can reference it without depending on
//! `holon-layout-testing`. This module just re-exports it for tests that
//! already pull in `holon-layout-testing`.

pub use holon_frontend::vms_button_id_for;

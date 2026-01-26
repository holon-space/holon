//! Shared scaffolding for the TUI integration tests.
//!
//! Both `tui_ui_pbt` and `screenshot_painter` include this module via
//! `mod common`, so cargo compiles it once per test target and warns
//! about items only used by the *other* target. `allow(dead_code)`
//! covers that — the items aren't actually unused in aggregate.
#![allow(dead_code)]

pub mod screenshot;
pub mod test_harness;

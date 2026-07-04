//! Shared gpui PBT harness modules for the composed windowed path.
//!
//! Increment 4c: the phased real-NSWindow shell (`run_in_gpui_window` +
//! `OnReady`/`GpuiLaunchContext`, the phased ready-protocol) and the
//! phased harnesses (`random_pbt`, `random_pbt_sim`, `windowed_replay`) were
//! DELETED. The survivors are the composed windowed boot/replay helper
//! (`windowed_wide`) and the TestPlatform `SimUserDriver`
//! (`sim_windowed_replay`); the windowed random runner is the 4b loop in
//! `gpui_composed_windowed_loop.rs`.
//!
//! Included via `#[path = "pbt_harness/mod.rs"] mod pbt_harness;` from each
//! test binary (subdir modules aren't auto-discovered as tests).

#![allow(dead_code)]

pub mod capture;
pub mod sim_windowed_replay;
pub mod windowed_wide;

pub use holon_integration_tests::libtest_list::handled_list_protocol;

/// Extract the panic message from a caught payload (`panic!`/`format!` →
/// `String`; `expect`/`&str` literals → `&str`).
pub fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<panic with non-string payload>".to_string()
    }
}

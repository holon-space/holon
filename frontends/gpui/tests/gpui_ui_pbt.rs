//! GPUI UI PBT — random geometry-based PBT against a REAL GPUI window with
//! xcap screenshots, via the shared `pbt_harness` shell. Differs from
//! `gpui_gherkin_replay` only in the runner: random generation instead of
//! fixture replay.
//!
//! `harness = false` (GPUI needs the main thread). Run with:
//!   cargo test -p holon-gpui --test gpui_ui_pbt
//! Tune step count with PBT_NUM_STEPS (default 50).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::phased::run_pbt_with_driver_sync_callback;

fn main() {
    let num_steps: u32 = std::env::var("PBT_NUM_STEPS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50);

    pbt_harness::run_in_gpui_window("Holon PBT", "gpui", move |driver, on_ready| {
        run_pbt_with_driver_sync_callback(num_steps, driver, on_ready)
    });
}

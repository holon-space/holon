//! GPUI capture replay — replays a machine-captured `Fixture<E2ETransition>`
//! JSON (the `tests/.captures/<slice>.captured.json` artifact a panicking PBT
//! writes) through a REAL GPUI window via the shared `pbt_harness` shell.
//!
//! This is the windowed twin of the headless lattice replay
//! (`bisection_pbt::replay_capture_verbose_from_env`): same capture file, but
//! driven through a live `InputState` / `GpuiUserDriver` instead of the headless
//! `BisectionStepper`. It exists because some failures are *runner-coupled* —
//! they only diverge when transitions route through the real editor widget path,
//! so the headless oracle never reproduces them and headless ddmin yields false
//! minima. Replaying a capture here reproduces the real failure deterministically
//! and is the faithful oracle a window-driven minimizer needs.
//!
//! Shares essentially all of its machinery with `gpui_gherkin_replay`: only the
//! fixture *loader* differs (JSON capture vs hand-authored `.feature`). The
//! windowed spine is `phased::replay_fixture_with_driver_sync_callback`, which
//! both binaries call.
//!
//! `harness = false` (GPUI needs the main thread). Run with:
//!   cargo test -p holon-gpui --test gpui_capture_replay
//! Override the capture with HOLON_CAPTURE=/abs/path.json (default: the
//! integration-tests `tests/.captures/gpui_ui_pbt.captured.json`).

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::fixtures::json;
use holon_integration_tests::pbt::phased::replay_fixture_with_driver_sync_callback;

fn capture_path() -> String {
    std::env::var("HOLON_CAPTURE").unwrap_or_else(|_| {
        // Default to the gpui_ui_pbt capture the random runner writes on panic.
        // The integration-tests crate owns the `.captures/` dir, so resolve it
        // relative to this gpui crate's manifest dir.
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/holon-integration-tests/tests/.captures/gpui_ui_pbt.captured.json"
        )
        .to_string()
    })
}

fn main() {
    let path = capture_path();
    let fixture = json::load_file(std::path::Path::new(&path));
    eprintln!(
        "[Holon Capture Replay] replaying {:?} ({} steps) from {path}",
        fixture.name,
        fixture.steps.len()
    );
    // Replay under the capture's recorded editor-shape flags — preconditions
    // consult them, so differing flags change the transition alphabet and
    // the capture rejects mid-replay.
    fixture.apply_recorded_env_flags();
    // Replay under the wiring the capture was recorded with (pre-header
    // captures lack it — fall back to Full).
    let wiring = fixture.wiring.unwrap_or_else(holon_pbt_core::Wiring::full);
    eprintln!("[Holon Capture Replay] wiring: {wiring:?}");
    let steps = fixture.steps;

    pbt_harness::run_in_gpui_window(
        "Holon Capture Replay",
        "gpui_capture",
        move |_driver, on_ready| {
            replay_fixture_with_driver_sync_callback(wiring, steps, on_ready, |_, _| {}, None)
        },
    );
}

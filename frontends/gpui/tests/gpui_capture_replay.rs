//! GPUI capture replay — replays a machine-captured `Fixture<E2ETransition>`
//! JSON (a `tests/.captures/*.captured.json` artifact) through a REAL GPUI
//! window via the composed windowed path (increment 4c: repointed off the
//! phased driver-sync replay spine onto
//! `with_windowed_wide_sut` + `replay_steps` over `ComposedSut<WideE2E>`).
//!
//! It exists because some failures are *runner-coupled* — they only diverge
//! when transitions route through the real window/driver path, so a headless
//! replay never reproduces them. Captures must be POST-BOOT (the composed
//! alphabet has no `StartApp`; the wide seed is the boot org) — a legacy
//! phased capture fails loud at the precondition assert.
//!
//! `harness = false`. Run with:
//!   cargo test -p holon-gpui --features pbt --test gpui_capture_replay
//! Select the capture with HOLON_CAPTURE=/abs/path.json (default: the
//! composed keystone's capture path). A missing capture is a DISCLOSED no-op
//! (there is nothing to replay), mirroring `fixtures::json::load_dir`.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::fixtures::json;
use pbt_harness::windowed_wide::replay_fixture_windowed;

fn capture_path() -> String {
    std::env::var("HOLON_CAPTURE").unwrap_or_else(|_| {
        // The integration-tests crate owns the `.captures/` dir, so resolve it
        // relative to this gpui crate's manifest dir.
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../crates/holon-integration-tests/tests/.captures/general_e2e_composed_pbt.\
             captured.json"
        )
        .to_string()
    })
}

fn main() {
    let path = capture_path();
    if !std::path::Path::new(&path).exists() {
        eprintln!("[Holon Capture Replay] {path} does not exist — no capture to replay");
        return;
    }
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

    if let Err(payload) = replay_fixture_windowed("gpui_capture_replay", &fixture) {
        std::panic::resume_unwind(payload);
    }
    eprintln!(
        "[Holon Capture Replay] PASS — capture replayed GREEN through the windowed \
         ComposedSut<WideE2E>"
    );
}

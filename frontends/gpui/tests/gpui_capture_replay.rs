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
//!   HOLON_CAPTURE=/abs/path.json cargo test -p holon-gpui --features pbt \
//!     --test gpui_capture_replay
//! Replay is OPT-IN: without `HOLON_CAPTURE` this is a disclosed no-op, so the
//! suite stays hermetic no matter what sits in the gitignored `.captures/`.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::fixtures::json;
use pbt_harness::windowed_wide::replay_fixture_windowed;

fn main() {
    if pbt_harness::handled_list_protocol("gpui_capture_replay") {
        return;
    }
    // A capture is by construction a RECORDED FAILING sequence, and `.captures/`
    // is gitignored — so replaying whatever happens to sit at a default path
    // would flip the suite red from state version control cannot see. Replay is
    // opt-in; an explicitly requested capture that is missing fails loud.
    let Ok(path) = std::env::var("HOLON_CAPTURE") else {
        eprintln!(
            "[Holon Capture Replay] no capture requested; set HOLON_CAPTURE=<path> to replay"
        );
        return;
    };
    assert!(
        std::path::Path::new(&path).exists(),
        "[Holon Capture Replay] HOLON_CAPTURE={path} does not exist"
    );
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

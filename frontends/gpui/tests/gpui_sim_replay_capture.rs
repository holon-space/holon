//! Deterministic sim (TestPlatform) replay of a single captured failing
//! sequence — a LOCKED, debuggable reproduction (no proptest generation, so the
//! exact same steps run every time). Pins the split-chain portion of the
//! `PressKey`→Loro windowed divergence (`presskey_loro_split_backspace.json`):
//! split a 2-char block at 1, then at 0 twice.
//!
//! Increment 4c: repointed off the phased `SimReplayer` onto the composed
//! windowed path (`with_windowed_wide_sut` + `replay_steps` over
//! `ComposedSut<WideE2E>`); the fixture is re-encoded POST-BOOT against the
//! wide seed (`block:c1`, same 2-char shape as the original "Q8" capture).
//! DISCLOSED EXCLUSION: the original PressKey backspace tail is
//! precondition-gated off the composed base (`full_headless` wiring has no
//! `Actor::UI` → `has_editor_buffer()` is false; the E4 keystroke input family
//! is a tracked Phase-3 blocker per the C-3 rung audit). Revive it from VCS
//! history once E4 lands.
//!
//! `harness = false`. Run with:
//!   cargo test -p holon-gpui --test gpui_sim_replay_capture --features pbt
//! Override the capture with `HOLON_CAPTURE=/abs/path.json`.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::fixtures::json;
use pbt_harness::windowed_wide::replay_fixture_windowed;

fn main() {
    let path = std::env::var("HOLON_CAPTURE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/presskey_loro_split_backspace.json"
        )
        .to_string()
    });
    let fixture = json::load_file(std::path::Path::new(&path));
    eprintln!(
        "[Holon Sim Replay Capture] replaying {:?} ({} steps) from {path}",
        fixture.name,
        fixture.steps.len()
    );
    fixture.apply_recorded_env_flags();
    if let Err(payload) = replay_fixture_windowed("gpui_sim_replay_capture", &fixture) {
        std::panic::resume_unwind(payload);
    }
    eprintln!(
        "[Holon Sim Replay Capture] PASS — locked sequence replayed GREEN through the windowed \
         ComposedSut<WideE2E>"
    );
}

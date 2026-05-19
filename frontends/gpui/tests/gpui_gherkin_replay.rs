//! GPUI Gherkin replay — replays a hand-authored `.feature` through a REAL
//! GPUI window via the shared `pbt_harness` shell. Differs from `gpui_ui_pbt`
//! only in the runner: fixture replay instead of random generation.
//!
//! `harness = false` (GPUI needs the main thread). Run with:
//!   cargo test -p holon-gpui --test gpui_gherkin_replay
//! Override the feature with GHERKIN_FEATURE=/abs/path.feature.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::fixtures::gherkin::GherkinFixtureSource;
use holon_integration_tests::pbt::fixtures::FixtureSource;
use holon_integration_tests::pbt::phased::replay_fixture_with_driver_sync_callback;

fn feature_path() -> String {
    std::env::var("GHERKIN_FEATURE").unwrap_or_else(|_| {
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/features/ordinary_block_interaction.feature"
        )
        .to_string()
    })
}

fn main() {
    let path = feature_path();
    let mut fixtures = GherkinFixtureSource::new(&path).load();
    assert_eq!(
        fixtures.len(),
        1,
        "gpui_gherkin_replay expects exactly one scenario in {path:?}, got {}",
        fixtures.len()
    );
    let fixture = fixtures.remove(0);
    eprintln!(
        "[Holon Gherkin Replay] replaying {:?} ({} steps) from {path}",
        fixture.name,
        fixture.steps.len()
    );
    let steps = fixture.steps;

    pbt_harness::run_in_gpui_window(
        "Holon Gherkin Replay",
        "gpui_gherkin",
        move |_driver, on_ready| replay_fixture_with_driver_sync_callback(steps, on_ready),
    );
}

//! GPUI Gherkin replay — replays a hand-authored `.feature` through a REAL
//! GPUI window via the composed windowed path (increment 4c: repointed off the
//! phased driver-sync replay spine onto
//! `with_windowed_wide_sut` + `replay_steps` over `ComposedSut<WideE2E>`).
//!
//! Features must be POST-BOOT and authored against the wide seed (the composed
//! alphabet has no `StartApp` / org-seed ceremony — the wide seed is the boot
//! org), the same convention as the headless `composed_split_gherkin` fixture.
//!
//! `harness = false`. Run with:
//!   cargo test -p holon-gpui --features pbt --test gpui_gherkin_replay
//! Override the feature with GHERKIN_FEATURE=/abs/path.feature.

#[path = "pbt_harness/mod.rs"]
mod pbt_harness;

use holon_integration_tests::pbt::fixtures::FixtureSource;
use holon_integration_tests::pbt::fixtures::gherkin::GherkinFixtureSource;
use pbt_harness::windowed_wide::replay_fixture_windowed;

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
    if pbt_harness::handled_list_protocol("gpui_gherkin_replay") {
        return;
    }
    let path = feature_path();
    let fixtures = GherkinFixtureSource::new(&path).load();
    assert!(
        !fixtures.is_empty(),
        "gpui_gherkin_replay expects at least one scenario in {path:?}"
    );
    for fixture in &fixtures {
        eprintln!(
            "[Holon Gherkin Replay] replaying {:?} ({} steps) from {path}",
            fixture.name,
            fixture.steps.len()
        );
        if let Err(payload) = replay_fixture_windowed("gpui_gherkin_replay", fixture) {
            std::panic::resume_unwind(payload);
        }
    }
    eprintln!(
        "[Holon Gherkin Replay] PASS — {} scenario(s) replayed GREEN through the windowed \
         ComposedSut<WideE2E>",
        fixtures.len()
    );
}

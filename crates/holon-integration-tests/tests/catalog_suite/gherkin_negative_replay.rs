//! Replay of the `_gherkin_negative/` controls — fixtures whose whole point is
//! that strict replay REFUSES them. A control that stops panicking is a
//! silent-degradation regression, so each is a `#[should_panic]` with the
//! phrase the refusal must carry.

#![cfg(feature = "pbt")]

use std::path::Path;
use std::path::PathBuf;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;

fn negative_feature(name: &str) -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/_gherkin_negative")
        .join(name);
    assert!(path.is_file(), "feature file missing: {}", path.display());
    path
}

/// D35.a item 3. A headless mode switch has no surface to act on, and the
/// refusal must name that rather than let the click degrade to a ghost-entity
/// focus.
#[test]
#[should_panic(expected = "preconditions FAILED for SwitchViewMode (NoModeSwitchableSurface)")]
fn switch_view_mode_without_a_switcher_is_refused() {
    holon_integration_tests::pbt::fixtures::run_feature_strict::<
        WideE2EMachine,
        ComposedSut<WideE2E>,
    >(negative_feature("switch_view_mode_no_surface.feature"));
}

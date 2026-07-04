//! Replay of `tests/fixtures/_gherkin_assert/parentage.feature` — the teeth for
//! the structural `Then` vocabulary (`block X is a child of block Y` /
//! `block X is a top-level block of Y`).
//!
//! Same composed stack as the dogfood recordings (`ComposedSut<WideE2E>` born
//! onto the wide seed), so the assertion reads the real store snapshot.

#![cfg(feature = "pbt")]

use std::path::Path;
use std::path::PathBuf;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;

fn parentage_feature() -> PathBuf {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/_gherkin_assert/parentage.feature");
    assert!(path.is_file(), "feature file missing: {}", path.display());
    path
}

/// The two templates are structurally distinct, so no step can resolve to both.
#[test]
fn parentage_templates_are_unambiguous() {
    let catalog = holon_integration_tests::pbt::fixtures::assert_steps::assert_step_catalog();
    assert_eq!(catalog.len(), 2, "assert-step catalog: {catalog:?}");
    holon_pbt_core::step_vocabulary::check_template_ambiguity(&catalog)
        .expect("assert-step templates must not be structurally ambiguous");
}

#[test]
fn parentage_feature_replays_over_the_composed_sut() {
    holon_integration_tests::pbt::fixtures::run_feature_strict::<
        WideE2EMachine,
        ComposedSut<WideE2E>,
    >(parentage_feature());
}

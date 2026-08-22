//! Replay of `tests/fixtures/_gherkin_assert/parentage.feature` — the teeth for
//! the structural `Then` vocabulary (`block X is a child of block Y` /
//! `block X is a top-level block of Y`), plus the ambiguity gate over the WHOLE
//! assert-step catalog (ordering, task state, fold state, link resolution).
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

/// Every assert-step template is structurally distinct, so no step can resolve
/// to two of them. The catalog GROWS as `Then` vocabulary is added, so this
/// gates the property rather than a template count — a count assertion would
/// red on every legitimate addition while catching no ambiguity at all.
#[test]
fn parentage_templates_are_unambiguous() {
    let catalog = holon_integration_tests::pbt::fixtures::assert_steps::assert_step_catalog();
    assert!(!catalog.is_empty(), "assert-step catalog is empty");
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

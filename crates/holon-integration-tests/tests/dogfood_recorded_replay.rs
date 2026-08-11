//! Replay of the `.feature` files hand-authored during a dogfood-explorer
//! session (`tests/fixtures/dogfood-recorded/`).
//!
//! Each file records ONE flow that was first driven live over the app's MCP
//! surface, then re-expressed in the derived step vocabulary. Replay runs the
//! composed `ComposedSut<WideE2E>` catalog every tick, so a scenario that the
//! live app satisfies but the composed stack does not is a real divergence.
//!
//! Features are born-booted onto the wide seed (`structural-page` →
//! `parent`/`c1`/`c2`) — no `Given an org file` / `the app is started`
//! ceremony, the same convention as `composed_split_gherkin`.

#![cfg(feature = "pbt")]

use std::path::Path;
use std::path::PathBuf;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;
use holon_integration_tests::pbt::transitions::E2ETransition;

fn recorded_features() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/dogfood-recorded");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "feature"))
        .collect();
    files.sort();
    assert!(
        !files.is_empty(),
        "no recorded features in {}",
        dir.display()
    );
    files
}

/// Cheap gate: every recorded action step resolves through the ONE generated
/// step registry. Runs without a SUT, so an unknown phrasing fails in
/// milliseconds instead of after a full composed boot.
#[test]
fn recorded_features_parse_through_the_registry() {
    for file in recorded_features() {
        let text = std::fs::read_to_string(&file).expect("read feature");
        // `And` inherits the previous step's type, so an `And` after a `Then`
        // is an assertion and belongs to `match_assertion`, not the registry.
        let mut in_then = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("Scenario") || line.starts_with("Feature") {
                in_then = false;
            }
            if line.starts_with("Then ") {
                in_then = true;
                continue;
            }
            let rest = match line.split_once(' ') {
                Some(("When", rest)) | Some(("Given", rest)) => {
                    in_then = false;
                    rest
                }
                Some(("And", rest)) if !in_then => rest,
                _ => continue,
            };
            E2ETransition::parse_step(rest.trim(), None)
                .unwrap_or_else(|e| panic!("{}: {rest:?}: {e}", file.display()));
        }
    }
}

#[test]
fn recorded_features_replay_over_the_composed_sut() {
    for file in recorded_features() {
        holon_integration_tests::pbt::fixtures::run_feature_strict::<
            WideE2EMachine,
            ComposedSut<WideE2E>,
        >(&file);
    }
}

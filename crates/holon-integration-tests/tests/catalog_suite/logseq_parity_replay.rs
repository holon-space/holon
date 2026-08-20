//! Replay of the LogSeq-parity corpus (`tests/fixtures/logseq-parity/`).
//!
//! Every feature carries a feature-level `@wip` tag, so the strict runner skips
//! the whole corpus and reports the skip count. Un-tag a scenario and the
//! runner executes it through `ComposedSut<WideE2E>` and requires it to pass.
//! See the directory `README.md` for the `@wip` contract.

#![cfg(feature = "pbt")]

use std::path::Path;
use std::path::PathBuf;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2EMachine;

fn parity_features() -> Vec<PathBuf> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/logseq-parity");
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.extension().is_some_and(|x| x == "feature"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no parity features in {}", dir.display());
    files
}

#[test]
fn logseq_parity_corpus_skips_while_wip() {
    for file in parity_features() {
        holon_integration_tests::pbt::fixtures::run_feature_strict::<
            WideE2EMachine,
            ComposedSut<WideE2E>,
        >(&file);
    }
}

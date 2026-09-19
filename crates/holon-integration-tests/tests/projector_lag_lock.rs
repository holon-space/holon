#![cfg(feature = "pbt")]
//! **The lock for the Loro→SQL read-your-own-write race in
//! `convert_block_to_page`.**
//!
//! `convert_block_to_page` mints page P and then re-homes P's children under
//! it. The child move's destination guard used to read P's existence from the
//! SQL projection, but under Loro authority the create lands in Loro and SQL is
//! written later by the `loro-outbound-reconcile` task. When that projection
//! had not run, the guard reported the parent absent and the compound failed
//! with `Parent not found: block:<P>`. On a quiet machine the reconcile task
//! won the race, so the defect only ever surfaced as an intermittent landing
//! gate red under load — 11 unlagged runs here never reproduced it, including 2
//! full-corpus runs at the same load average as the failing gate.
//!
//! This test removes the machine from the equation.
//! `HOLON_TEST_PROJECTOR_LAG_MS` holds the projection back at
//! `LoroSyncController::on_loro_changed`, so the window the bug needs is open
//! by construction rather than by luck, and the hand-authored case that
//! exercises the convert replays through it.
//!
//! ## Why this binary holds exactly ONE test
//!
//! The lag is configured by a process-global environment variable, read by the
//! projector on every pass. A second test in this binary would therefore run
//! under the lag too, whether or not it wants to — and that is not theoretical:
//! at 600 ms the lag independently reds
//! `echo_loop_block_to_page_child_render_leak_parked` in the hand-authored
//! binary, on an `inv-sql-budget` ratchet inflated by lag-induced re-reads. So
//! the variable is set here, inside the one test, before any engine boots, and
//! this file must stay a single test. Cargo gives each `tests/*.rs` its own
//! binary and its own process, so nothing outside this file observes the
//! variable: it is never exported to a child, and no other test binary sets it.
//!
//! Red-first evidence and the full root cause are in the lane report
//! `journals-rewrite-race`; the escape is bugfunnel entry
//! `2026-09-19-convert-block-to-page-move-races-loro-sql-projection`.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::hand_authored::load_cases;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

/// The convert-bearing case: its transition 9/22 is
/// `BlockToPage(journals::auto-create)`, the step that mints P and then moves
/// children under it.
const CASE: &str = "journals-external-rewrite-strips-convert-link-marks";

/// How long to hold the Loro→SQL projection back. 600 ms is far wider than the
/// race needs; the point is determinism, not calibration. Reverting the guard's
/// authority reads makes this test RED at this value.
const LAG_MS: &str = "600";

#[test]
fn convert_block_to_page_survives_a_lagging_projection() {
    // Both variables are set before anything boots: the projector reads the lag
    // on its first pass, and `load_cases` reads the filter immediately below.
    // SAFETY: single-threaded test entry, before any engine, runtime or
    // projector task exists, and this binary holds no other test that could be
    // running concurrently (see the module docs).
    unsafe {
        std::env::set_var("HOLON_TEST_PROJECTOR_LAG_MS", LAG_MS);
        std::env::set_var(
            holon_integration_tests::pbt::hand_authored::CASE_FILTER_ENV,
            CASE,
        );
    }

    let cases = load_cases();
    assert_eq!(
        cases.len(),
        1,
        "expected the filter to select exactly the one convert-bearing case, got {:?}",
        cases.iter().map(|c| &c.name).collect::<Vec<_>>()
    );
    let case = cases.into_iter().next().expect("one case");
    assert_eq!(case.name, CASE, "the filter selected the wrong case");

    let provenance = format!("projector-lag lock, case {:?}", case.name);
    let initial_state = match case.initial_state {
        Some(pinned) => pinned.into_reference_state(&provenance),
        None => wide_e2e_ref(),
    };
    // Same guard the hand-authored runner applies: a byte-identical
    // `initial_state` still replays differently under different capture flags,
    // so compare the recorded environment instead of assuming it.
    if let Some(report) = case
        .environment
        .mismatch_report(Some(&initial_state.harness.wiring))
    {
        panic!(
            "{provenance}: recorded capture environment does not match this replay — the \
             starting state differs from the one this case pins:\n{report}"
        );
    }

    // A fixed sequence, never generated or shrunk, so only `verbose` matters —
    // it makes `test_sequential` disclose each applied transition on stderr,
    // which is what names transition 9 when this goes red.
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    ComposedSut::<WideE2E>::test_sequential(config, initial_state, case.transitions, None);
}

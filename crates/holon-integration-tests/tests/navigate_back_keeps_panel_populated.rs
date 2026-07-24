#![cfg(feature = "pbt")]
//! **NavigateBack must keep the main panel populated (keystone regression).**
//!
//! Reproduces the keystone composed PBT's creation-slot panic
//! (`commit_creation_slot: main panel … resolves NO creation parent after 3s
//! (0 live rows)`) through the EXACT keystone harness
//! (`ComposedSut::<WideE2E>::test_sequential`) with a deterministic transition
//! sequence, so it is one flag-flip from a live
//! `hand-authored-regressions/keystone.jsonl` case.
//!
//! ROOT CAUSE (diagnosed, Inc 0 — see `crates/holon-turso/tests/
//! chained_matview_cdc_repro.rs` for the minimized reproducer that REFUTED the
//! earlier chained-matview / non-atomic-reseed theories):
//! `focus_roots` is the matview `navigation_history WHERE closed_at IS NULL`
//! (open rows only); forward navigation's `focus_replace` CLOSES the prior open
//! row. The main-panel focus query joins
//! `focus_roots fr JOIN navigation_cursor nc ON nc.history_id = fr.history_id`.
//! `NavigateBack` (`go_back` → `UPDATE navigation_cursor SET history_id =
//! <prior>`) moves the cursor onto a now-CLOSED history row, absent from
//! `focus_roots`, so the join legitimately yields 0 rows and the focus matview
//! retracts its entire result set — the main panel goes blank and never
//! re-asserts. The next `CreateBlockUnderFocus`'s `commit_creation_slot` then
//! sees 0 live rows and fails loud. This is a holon-side navigation write-side
//! consistency bug, NOT Turso IVM / chained-matview / reseed.
//!
//! FIX (ruled option (a), write-side invariant): `go_back`/`go_forward` re-open
//! the target row (`closed_at → NULL`) and close the departed row in ONE
//! transaction, so the cursor always points at an OPEN `navigation_history` row
//! (or has no row / a home NULL cursor). This test is the red→green lock.
//!
//! Deterministic sequence: create two doc pages, focus each (building
//! back-history and CLOSING the first page's row via focus_replace), then
//! `NavigateBack` onto that closed row, then drive the creation-slot gesture.
//! Before the fix: `NavigateBack` blanks the panel → the create panics with the
//! `0 live rows` signature. After the fix: the panel stays populated → the
//! block is created under the re-focused page and every invariant holds.

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::transitions::E2ETransition;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

/// Canonical `Fixture<E2ETransition>` JSONL for the deterministic repro.
const CASE: &str = r#"{"name": "navigate-back-keeps-panel-populated", "description": "focus two pages, NavigateBack onto the (focus_replace-closed) prior page, then create under focus — go_back must re-open the target so the focus_roots/navigation_cursor join stays satisfied and the panel is not blank (0 live rows)", "transitions": [{"CreateDocument": {"file_name": "doc_0.org"}}, {"CreateDocument": {"file_name": "doc_1.org"}}, {"NavigateFocus": {"region": "main", "block_id": "block:ref-doc-0"}}, {"NavigateFocus": {"region": "main", "block_id": "block:ref-doc-1"}}, {"NavigateBack": {"region": "main"}}, {"CreateBlockUnderFocus": {"content": "x", "id": null}}]}"#;

#[test]
fn navigate_back_keeps_panel_populated() {
    let case: holon_pbt_core::fixture::Fixture<E2ETransition> =
        serde_json::from_str(CASE).expect("navigate-back case must parse");
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    let initial_state = wide_e2e_ref();
    ComposedSut::<WideE2E>::test_sequential(config, initial_state, case.transitions, None);
}

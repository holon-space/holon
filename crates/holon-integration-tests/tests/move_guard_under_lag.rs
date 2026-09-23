#![cfg(feature = "pbt")]
//! `convert_block_to_page` re-homes the origin's children under the page it
//! just minted. Each child move passes the net gate, whose destination check
//! locates the page through the SQL projection; under Loro authority that
//! projection trails the create, so the gate must wait for it rather than
//! report the page absent.
//!
//! ## Why this binary holds exactly ONE test
//!
//! Same reason as `projector_lag_lock.rs`: the lag is a process-global
//! environment variable read by the projector on every pass, so a second test
//! here would silently run under it.
//!
//! @pbt kind harness
//! @pbt covers move-guard-destination-under-lag — the net gate's destination
//! check sees a page the running compound created before the projection has

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::hand_authored::HandAuthoredCase;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

/// Far wider than the race needs; the point is determinism, not calibration.
const LAG_MS: &str = "600";

#[test]
fn convert_block_to_page_moves_children_under_a_page_the_projection_has_not_reached() {
    // SAFETY: single-threaded test entry, before any engine, runtime or
    // projector task exists, and this binary holds no other test (module docs).
    unsafe {
        std::env::set_var("HOLON_TEST_PROJECTOR_LAG_MS", LAG_MS);
    }

    let line = r#"{"name": "convert-moves-a-child-under-lag", "transitions": [{"CreateBlockUnderFocus": {"content": "origin", "id": "block:lagorigin"}}, {"CreateBlockUnderFocus": {"content": "child", "id": "block:lagchild"}}, {"Indent": {"block_id": "block:lagchild"}}, {"BlockToPage": {"origin_id": "block:lagorigin"}}]}"#;
    let case: HandAuthoredCase = serde_json::from_str(line).expect("the lag case must parse");
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    ComposedSut::<WideE2E>::test_sequential(config, wide_e2e_ref(), case.transitions, None);
}

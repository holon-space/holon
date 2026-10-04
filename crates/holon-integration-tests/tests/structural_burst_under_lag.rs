#![cfg(feature = "pbt")]
//! Tab ×2 then Shift-Tab ×2 on one block, pressed with no settle between keys.
//! Each key must decide on the parent and sibling the keys before it produced.
//! Under Loro authority the SQL projection trails those keys, so a provider
//! that reads its decision rows from the projection refuses a legal outdent or
//! moves the block out of a parent it already left.
//!
//! No key in the burst is a legal refusal: prod logs every refused dispatch as
//! an ERROR, which `inv-no-observed-errors` reports.
//!
//! ## Why this binary holds exactly ONE test
//!
//! Same reason as `projector_lag_lock.rs`: the lag is a process-global
//! environment variable read by the projector on every pass, so a second test
//! here would silently run under it.
//!
//! @pbt kind harness
//! @pbt covers structural-burst-decides-on-the-write-authority — indent and
//! outdent decide on the write authority while the projection lags

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::hand_authored::HandAuthoredCase;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

const LAG_MS: &str = "600";

#[test]
fn tabs_then_shift_tabs_without_settling_decide_on_the_write_authority() {
    // SAFETY: single-threaded test entry, before any engine, runtime or
    // projector task exists, and this binary holds no other test (module docs).
    unsafe {
        std::env::set_var("HOLON_TEST_PROJECTOR_LAG_MS", LAG_MS);
    }

    let tab = r#"{"Indent": "block:burstx"}"#;
    let shift_tab = r#"{"Outdent": "block:burstx"}"#;
    let round = [tab, tab, shift_tab, shift_tab].join(", ");
    let line = format!(
        r#"{{"name": "structural-burst-under-lag", "transitions": [
            {{"CreateBlockUnderFocus": {{"content": "a", "id": "block:bursta"}}}},
            {{"CreateBlockUnderFocus": {{"content": "b", "id": "block:burstb"}}}},
            {{"CreateBlockUnderFocus": {{"content": "x", "id": "block:burstx"}}}},
            {{"Indent": {{"block_id": "block:burstb"}}}},
            {{"StructuralKeyBurst": {{"keys": [{round}, {round}]}}}}
        ]}}"#
    );
    let case: HandAuthoredCase = serde_json::from_str(&line).expect("the burst case must parse");
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    ComposedSut::<WideE2E>::test_sequential(config, wide_e2e_ref(), case.transitions, None);
}

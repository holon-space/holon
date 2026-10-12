#![cfg(feature = "pbt")]
//! A remote-list round decides its writes against the local rows. Those rows
//! must come from the type's write authority: read from a SQL table that has
//! not caught up with the previous round's create, the item looks absent and
//! the next round creates it again, which the Loro authority refuses as
//! ALREADY HELD.
//!
//! ## Why this binary holds exactly ONE test
//!
//! Same reason as `projector_lag_lock.rs`: the lag is a process-global
//! environment variable read by the projector on every pass, so a second test
//! here would silently run under it.
//!
//! @pbt kind harness
//! @pbt covers remote-list-read-authority — the idempotence round reads the
//! rows the previous round created from the write authority under projection
//! lag

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::hand_authored::load_cases;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

const CASE: &str = "remote-list-second-round-reads-the-write-authority";

/// Far wider than the race needs; the point is determinism, not calibration.
const LAG_MS: &str = "600";

#[test]
fn the_second_round_sees_the_item_the_first_round_created() {
    // SAFETY: single-threaded test entry, before any engine, runtime or
    // projector task exists, and this binary holds no other test (module docs).
    unsafe {
        std::env::set_var("HOLON_TEST_PROJECTOR_LAG_MS", LAG_MS);
        std::env::set_var(
            holon_integration_tests::pbt::hand_authored::CASE_FILTER_ENV,
            CASE,
        );
    }

    let cases = load_cases();
    assert_eq!(
        cases.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(),
        vec![CASE],
        "the filter must select exactly the one remote-list case"
    );
    let case = cases.into_iter().next().expect("one case");
    assert!(
        case.initial_state.is_none(),
        "the case replays on the default wide wiring, which holds the Loro authority"
    );
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    ComposedSut::<WideE2E>::test_sequential(config, wide_e2e_ref(), case.transitions, None);
}

#![cfg(feature = "pbt")]
//! Hand-authored cases and replay settings that are refused by name. Each test
//! panics on purpose, so these run in a process of their own: a panic raised
//! beside a replay would land in that replay's problem sink.

#[path = "hand_authored_fakes.rs"]
mod hand_authored_fakes;

use std::collections::BTreeSet;

use hand_authored_fakes::CountingRef;
use hand_authored_fakes::four_transition_case;
use holon_integration_tests::pbt::hand_authored::Boot;
use holon_integration_tests::pbt::hand_authored::CapturedInitialState;
use holon_integration_tests::pbt::hand_authored::CheckScope;
use holon_integration_tests::pbt::hand_authored::LockedReplay;
use holon_integration_tests::pbt::hand_authored::ReplayCase;
use holon_integration_tests::pbt::hand_authored::parse_case;
use holon_integration_tests::pbt::hand_authored::parse_replay_case;
use holon_integration_tests::pbt::hand_authored::replay;
use proptest_state_machine::StateMachineTest;

/// A field the schema does not know is a LOUD parse failure, not a silently
/// defaulted replay of a different starting state.
#[test]
#[should_panic(expected = "failed to parse")]
fn unknown_initial_state_field_is_loud() {
    let line = r#"{"name": "n", "transitions": [], "initial_state": {"wiring": {"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}, "seed": 7}}"#;
    parse_case(std::path::Path::new("<inline>"), 1, line);
}

/// A wiring axis silently dropped by serde would make the replayed base differ
/// from the authored one; the round-trip check catches it at ANY nesting depth.
#[test]
#[should_panic(expected = "does not round-trip")]
fn nested_unknown_wiring_field_is_loud() {
    let line = r#"{"name": "n", "transitions": [], "initial_state": {"wiring": {"storage_adapters": ["Turso"], "sync_adapters": [], "actors": [], "peers": ["x"]}}}"#;
    parse_case(std::path::Path::new("<inline>"), 1, line);
}

/// An `init_state` draw is always valid (`any_valid_wiring` filters), so an
/// invalid manifest means the case was hand-typed wrong — fail loud rather than
/// boot a wiring no real run could have produced.
#[test]
#[should_panic(expected = "INVALID wiring manifest")]
fn invalid_pinned_wiring_is_loud() {
    let pinned: CapturedInitialState = serde_json::from_str(
        r#"{"wiring": {"storage_adapters": [], "sync_adapters": [], "actors": []}}"#,
    )
    .expect("shape parses");
    pinned.into_reference_state("<inline>");
}

/// The hole the inner schema guards structurally cannot see: `Fixture` is not
/// `deny_unknown_fields`, so a typo in the KEY leaves `initial_state = None`
/// and the round-trip check compares `Null` against `Null`. Only the
/// `hand_authored::KNOWN_CASE_KEYS` allowlist catches it.
#[test]
#[should_panic(expected = "unknown top-level key \"initail_state\"")]
fn mistyped_initial_state_key_is_loud() {
    let line = r#"{"name": "n", "transitions": [], "initail_state": {"wiring": {"storage_adapters": ["Turso"], "sync_adapters": [], "actors": []}}}"#;
    parse_case(std::path::Path::new("<inline>"), 1, line);
}


#[test]
#[should_panic(expected = "case \"n\": `also_check_after` names transition 4, out of range 1..3")]
fn also_check_after_the_last_transition_is_refused() {
    parse_replay_case(
        std::path::Path::new("<inline>"),
        1,
        &four_transition_case(r#", "also_check_after": [4]"#),
    );
}

#[test]
#[should_panic(expected = "case \"n\": `also_check_after` holds \"2\", not a transition number")]
fn malformed_also_check_after_is_refused() {
    parse_replay_case(
        std::path::Path::new("<inline>"),
        1,
        &four_transition_case(r#", "also_check_after": ["2"]"#),
    );
}

#[test]
#[should_panic(expected = "case \"no-locks\" has no `locks` field")]
fn a_case_without_locks_is_refused_by_name() {
    parse_replay_case(
        std::path::Path::new("<inline>"),
        1,
        r#"{"name": "no-locks", "transitions": []}"#,
    );
}

#[test]
#[should_panic(expected = "case \"n\": `locks` is empty")]
fn empty_locks_are_refused() {
    parse_replay_case(
        std::path::Path::new("<inline>"),
        1,
        r#"{"name": "n", "locks": [], "transitions": []}"#,
    );
}

/// Engaged at no checkpoint, so the case judges nothing it locks.
struct VacuousSut;

impl StateMachineTest for VacuousSut {
    type SystemUnderTest = ();
    type Reference = CountingRef;

    fn init_test(_: &usize) {}

    fn apply(sut: (), _: &usize, _: ()) {
        sut
    }

    fn check_invariants(_: &(), _: &usize) {}
}

impl LockedReplay for VacuousSut {
    fn check_locked(_: &(), _: &usize, _: &CheckScope) -> BTreeSet<&'static str> {
        BTreeSet::from(["inv-no-errors"])
    }

    fn teardown_locked(_: (), _: usize, _: &CheckScope) -> BTreeSet<&'static str> {
        BTreeSet::new()
    }
}

#[test]
#[should_panic(expected = "locked invariant(s) [\"inv-x\"] were `Skipped` at every checkpoint")]
fn a_lock_skipped_at_every_checkpoint_is_refused() {
    let ReplayCase {
        checkpoints, locks, ..
    } = parse_replay_case(
        std::path::Path::new("<inline>"),
        1,
        r#"{"name": "n", "locks": ["inv-x"], "transitions": []}"#,
    );
    replay::<VacuousSut>(
        "n",
        "stub",
        0,
        Vec::new(),
        &checkpoints,
        &locks,
        Boot::Fresh,
    );
}


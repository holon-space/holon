#![cfg(feature = "pbt")]
//! Hand-authored cases and replay settings that are refused by name. Each test
//! panics on purpose, so these run in a process of their own: a panic raised
//! beside a replay would land in that replay's problem sink.

#[path = "hand_authored_fakes.rs"]
mod hand_authored_fakes;

use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;

use hand_authored_fakes::CountingRef;
use hand_authored_fakes::four_transition_case;
use holon_integration_tests::pbt::composed::observed_errors::InvNoObservedErrors;
use holon_integration_tests::pbt::composed::observed_errors::ObservedProblems;
use holon_integration_tests::pbt::hand_authored::Boot;
use holon_integration_tests::pbt::hand_authored::CapturedInitialState;
use holon_integration_tests::pbt::hand_authored::CheckScope;
use holon_integration_tests::pbt::hand_authored::LockedReplay;
use holon_integration_tests::pbt::hand_authored::ReplayCase;
use holon_integration_tests::pbt::hand_authored::parse_case;
use holon_integration_tests::pbt::hand_authored::parse_replay_case;
use holon_integration_tests::pbt::hand_authored::replay;
use holon_pbt_core::capabilities::RefCopies;
use holon_pbt_core::capabilities::WriteHeld;
use holon_pbt_core::composition::CapMap;
use holon_pbt_core::invariant::Invariant;
use holon_pbt_core::invariant::InvariantResult;
use proptest::prelude::Just;
use proptest::prelude::Strategy;
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
    fn judge_problems(_: &(), _: &usize) -> Result<(), String> {
        Ok(())
    }

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

thread_local! {
    /// The transition (1-based) at which [`ProblemSut`] logs a problem; 0 logs none.
    static LOGS_AT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static PROBLEM: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
    /// The files whose write-back the reference of [`ProblemSut`] holds armed.
    static HELD: std::cell::RefCell<BTreeSet<String>> = const { std::cell::RefCell::new(BTreeSet::new()) };
}

struct ArmedRef(BTreeSet<String>);

impl RefCopies for ArmedRef {
    fn model_copies(&self) -> Vec<holon_pbt_core::capabilities::ModelCopy> {
        Vec::new()
    }

    fn write_held(&self) -> WriteHeld {
        WriteHeld {
            files: self.0.clone(),
            blocks: BTreeSet::new(),
        }
    }

    fn crash_restore_copies(&self) -> std::collections::BTreeMap<String, Vec<Vec<String>>> {
        Default::default()
    }
}

struct Logged(Vec<String>);

impl ObservedProblems for Logged {
    fn observed_problems(&self) -> Vec<String> {
        self.0.clone()
    }
}

/// Logs [`PROBLEM`] during transition [`LOGS_AT`] and nothing during the
/// others, like the span collector's problem sink, which each transition start
/// empties.
struct ProblemSut;

impl StateMachineTest for ProblemSut {
    type SystemUnderTest = Vec<String>;
    type Reference = CountingRef;

    fn init_test(_: &usize) -> Vec<String> {
        Vec::new()
    }

    fn apply(_: Vec<String>, applied: &usize, _: ()) -> Vec<String> {
        if LOGS_AT.get() == *applied {
            vec![PROBLEM.with_borrow(Clone::clone)]
        } else {
            Vec::new()
        }
    }

    fn check_invariants(_: &Vec<String>, _: &usize) {}
}

impl LockedReplay for ProblemSut {
    fn check_locked(sut: &Vec<String>, applied: &usize, _: &CheckScope) -> BTreeSet<&'static str> {
        if let Err(problems) = Self::judge_problems(sut, applied) {
            panic!("[inv-no-observed-errors] after transition {applied}: {problems}");
        }
        BTreeSet::from(["inv-x", "inv-sql-budget"])
    }

    fn judge_problems(sut: &Vec<String>, _: &usize) -> Result<(), String> {
        let mut caps = CapMap::new();
        caps.insert(
            std::sync::Arc::new(Logged(sut.clone())) as std::sync::Arc<dyn ObservedProblems>
        );
        let mut reference = CapMap::new();
        reference.insert(
            std::sync::Arc::new(ArmedRef(HELD.with_borrow(Clone::clone)))
                as std::sync::Arc<dyn RefCopies>,
        );
        match futures::executor::block_on(InvNoObservedErrors.check(&reference, &caps)) {
            InvariantResult::Fail(message) => Err(message),
            _ => Ok(()),
        }
    }

    fn teardown_locked(_: Vec<String>, _: usize, _: &CheckScope) -> BTreeSet<&'static str> {
        BTreeSet::new()
    }
}

const REAL_PROBLEM: &str = "[ERROR] holon_core::store (crates/holon-core/src/store.rs:1): boom";
const FILTERED_PROBLEM: &str = "[ERROR] iroh::socket::remote_map::remote_state \
    (/registry/iroh-1.4.1/src/socket/remote_map/remote_state.rs:723): \
    could not close last open path";
const ARMED_REFUSAL: &str = "[ERROR] holon_org::writer (crates/holon-org/src/writer.rs:1): \
    injected failure between temp write and rename (in-memory): /vault/held.org";

/// Replays `n` transitions of which transition `logs_at` (1-based, 0 for none)
/// logs `problem`, against a reference that holds `held` armed; returns the
/// replay's panic message, if any.
fn replay_message(
    n: usize,
    logs_at: usize,
    problem: &str,
    held: &[&str],
    also_check_after: &BTreeSet<usize>,
    lock: &str,
) -> Option<String> {
    let also = if also_check_after.is_empty() {
        String::new()
    } else {
        format!(
            r#", "also_check_after": {:?}"#,
            also_check_after.iter().collect::<Vec<_>>()
        )
    };
    let create = r#"{"CreateBlockUnderFocus": {"content": "c", "id": "block:c"}}"#;
    let line = format!(
        r#"{{"name": "n", "locks": ["{lock}"], "transitions": [{}]{also}}}"#,
        vec![create; n].join(", ")
    );
    let ReplayCase {
        case,
        checkpoints,
        locks,
        ..
    } = parse_replay_case(std::path::Path::new("<inline>"), 1, &line);
    LOGS_AT.set(logs_at);
    PROBLEM.set(problem.to_string());
    HELD.set(held.iter().map(|f| f.to_string()).collect());
    let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
        replay::<ProblemSut>(
            &case.name,
            "stub",
            0,
            vec![(); n],
            &checkpoints,
            &locks,
            Boot::Fresh,
        )
    }));
    outcome.err().map(|payload| {
        payload
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
            .expect("the replay panics with a message")
    })
}

fn assert_problem_judged(n: usize, p: usize, also_check_after: &BTreeSet<usize>, lock: &str) {
    let message = replay_message(n, p, REAL_PROBLEM, &[], also_check_after, lock)
        .unwrap_or_else(|| panic!("replay did not panic: n={n} p={p} also={also_check_after:?}"));
    assert!(
        message.contains("inv-no-observed-errors")
            && message.contains(&format!("after transition {p}:")),
        "n={n} p={p} also={also_check_after:?}: {message}"
    );
    assert_eq!(
        replay_message(n, 0, REAL_PROBLEM, &[], also_check_after, lock),
        None,
        "a replay that logs nothing passes"
    );
}

#[test]
fn a_problem_logged_at_a_transition_without_a_check_is_judged() {
    for (n, p, also, lock) in [
        (3, 1, vec![], "inv-x"),
        (4, 2, vec![3], "inv-x"),
        (3, 3, vec![], "inv-x"),
        (4, 2, vec![], "inv-sql-budget"),
    ] {
        assert_problem_judged(n, p, &also.into_iter().collect(), lock);
    }
}

#[test]
fn a_problem_the_invariant_filters_stays_green() {
    for p in 1..=3 {
        assert_eq!(
            replay_message(3, p, FILTERED_PROBLEM, &[], &BTreeSet::new(), "inv-x"),
            None
        );
    }
}

#[test]
fn a_write_the_reference_holds_armed_stays_green() {
    for p in 1..=3 {
        assert_eq!(
            replay_message(
                3,
                p,
                ARMED_REFUSAL,
                &["held.org"],
                &BTreeSet::new(),
                "inv-x"
            ),
            None
        );
        let message = replay_message(
            3,
            p,
            ARMED_REFUSAL,
            &["other.org"],
            &BTreeSet::new(),
            "inv-x",
        )
        .expect("a refusal on a file the reference does not hold is judged");
        assert!(message.contains("held.org"), "{message}");
    }
}

proptest::proptest! {
    #[test]
    fn every_transition_is_judged_for_problems(
        (n, p, also) in (1usize..8).prop_flat_map(|n| (
            Just(n),
            1..=n,
            proptest::collection::btree_set(1..n.max(2), 0..n).prop_map(move |s| {
                s.into_iter().filter(|k| *k < n).collect::<BTreeSet<usize>>()
            }),
        ))
    ) {
        assert_problem_judged(n, p, &also, "inv-x");
    }
}

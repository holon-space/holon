#![cfg(feature = "pbt")]
//! **Hand-authored keystone regressions — inject a known repro without waiting
//! for the random generator to find it.**
//!
//! The standard proptest regression file
//! (`general_e2e_composed_pbt.proptest-regressions`) persists an RNG **seed**
//! (`cc <hex>`), not the concrete case: on replay proptest reseeds its RNG and
//! re-runs the WHOLE strategy generation. The human-readable `# shrinks to …`
//! comment is informational — it is NOT parsed. So a seed regression is NOT
//! hand-authorable: you would need the inverse of the generator to find a seed
//! that produces the sequence you want.
//!
//! This test provides the missing injection path. It reads a sidecar of
//! hand-authored cases — [`DEFAULT_SIDECAR`], one JSON object per line — where
//! each case is a **serialized `Vec<E2ETransition>`** (the production
//! transition enum derives `serde`) plus an optional
//! [`CapturedInitialState`]. It then drives each case through the EXACT
//! keystone harness (`ComposedSut::<WideE2E>`): the same SUT boot, the same
//! production transition `apply`, and the same composed invariant catalog the
//! random keystone runs. Three things differ from the random keystone: the
//! `(initial_state, transitions)` pair comes from the JSONL instead of a
//! `proptest` draw, only the invariants the case names in `locks` plus
//! `hand_authored::LOCK_FLOOR` run, and they run only just before and after the
//! case's last transition and any it names in `also_check_after`
//! (`hand_authored::replay`). The selection floor, the engagement ledger and
//! the shape reach floor are the random keystone's; the replay only prints the
//! shape reach, which `just keystone-smoke` asserts for its shape case.
//!
//! ## `initial_state` is the drawn wiring; `environment` is the rest
//!
//! `WideE2EMachine::init_state` draws exactly ONE value, a
//! `holon_pbt_core::wiring::Wiring` manifest, and maps it through
//! `wide_e2e_ref_for`. So capturing the manifest captures the whole
//! *generated* half of the starting state — no partial `ReferenceState`
//! serialization (with its `Arc<ShadowInterpreter>`, live Loro peers and
//! runtime handles) is needed or possible.
//!
//! `wide_e2e_ref_for` is NOT a pure function of that manifest, though. It also
//! reads process env: `HOLON_FOLDER_COMPANION_SEED` (via
//! `wide_e2e::folder_companion_enabled`) makes it seed extra blocks, and
//! `PBT_MUTABLE_TEXT` (via `ReferenceState::mutable_text_enabled`) changes the
//! reference's editor semantics. Both are in
//! `holon_pbt_core::fixture::CAPTURE_ENV_FLAGS`, and the `Fixture::environment`
//! field records them; the driver compares the recording against the live env
//! and panics on any difference, so a case captured under one flag set can
//! never silently replay as something else.
//!
//! A case that omits `initial_state` replays over [`wide_e2e_ref`] (the
//! `full_headless` wiring, identical to `HOLON_PBT_FORCE_FULL=1`) exactly as
//! before.
//!
//! ## Env seams
//!
//! * `HOLON_HAND_AUTHORED_SIDECAR` — replace the sidecar path (absolute, or
//!   relative to the crate root). Lets a cross-revision A/B probe point two
//!   trees at ONE out-of-tree case file instead of editing each tree's
//!   committed JSONL.
//! * `HOLON_HAND_AUTHORED_CASE` — comma-separated list of case `name`s to run.
//! * `HOLON_HAND_AUTHORED_SKIP` — comma-separated list of case `name`s to
//!   QUARANTINE (skip a known-bad case without editing the committed sidecar,
//!   so un-quarantining can never be forgotten in a code edit). Each skip is
//!   disclosed LOUD on stderr plus a summary count, so a green can never read
//!   as full coverage. An unknown name is a hard error, never a silent no-op.
//!
//! Fail LOUD (parse-don't-validate): an unreadable sidecar, a malformed line,
//! an unknown transition variant, a non-round-tripping `initial_state`, an
//! invalid wiring manifest, or a name filter that matches nothing panics with
//! the offending file:line — never a silent skip. A case that trips a
//! production defect or an invariant fails the test exactly as it would in the
//! random keystone.
//!
//! See `docs/Testing/HandAuthoredRegressions.md` for the authoring procedure
//! and the limits of what a hand-authored regression can express.

use std::collections::BTreeSet;
use std::panic::AssertUnwindSafe;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::span_metrics::ComposedBudget;
use holon_integration_tests::pbt::composed::span_metrics::ComposedSpanMetrics;
use holon_integration_tests::pbt::composed::span_metrics::SutMetricsLifecycle;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::hand_authored::CapturedInitialState;
use holon_integration_tests::pbt::hand_authored::CheckScope;
use holon_integration_tests::pbt::hand_authored::Checkpoints;
use holon_integration_tests::pbt::hand_authored::HandAuthoredCase;
use holon_integration_tests::pbt::hand_authored::LockedReplay;
use holon_integration_tests::pbt::hand_authored::Locks;
use holon_integration_tests::pbt::hand_authored::ReplayCase;
use holon_integration_tests::pbt::hand_authored::load_replay_cases;
use holon_integration_tests::pbt::hand_authored::parse_case;
use holon_integration_tests::pbt::hand_authored::parse_replay_case;
use holon_integration_tests::pbt::hand_authored::replay;
use holon_integration_tests::pbt::hand_authored::sidecar_path;
use holon_integration_tests::pbt::invariants::bodies::shape_sim_matches_authority;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_integration_tests::pbt::transitions::UndoLastMutation;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

/// Replay every hand-authored keystone regression through the production
/// harness. A case that reproduces a live defect (or breaks an invariant) fails
/// here exactly as the random keystone would — and becomes a permanent lock:
/// green once the defect is fixed. A red case does not stop the replay: the
/// test fails after the last case and names every red one.
#[test]
fn hand_authored_keystone_regressions() {
    let cases = load_replay_cases();
    assert!(
        !cases.is_empty(),
        "no hand-authored regression cases found in {:?} — the sidecar must not be empty",
        sidecar_path()
    );

    let mut red = Vec::new();
    for ReplayCase {
        case,
        checkpoints,
        locks,
        known_red,
    } in cases
    {
        let name = case.name.clone();
        let known_red = known_red.map_or(String::new(), |row| format!("known red: {row}, "));
        eprintln!(
            "[hand-authored regression] running case {name:?} ({known_red}{} transitions, \
             {checkpoints:?}, {locks:?})",
            case.transitions.len()
        );
        // The panic hook has already printed this case's panic block; catching it
        // lets every later case be judged too.
        match std::panic::catch_unwind(AssertUnwindSafe(|| replay_case(case, &checkpoints, &locks)))
        {
            Ok(()) => eprintln!("[hand-authored regression] PASSED case {name:?}"),
            Err(_) => {
                eprintln!("[hand-authored regression] RED case {name:?}");
                red.push(name);
            }
        }
    }
    shape_sim_matches_authority::report_reach();
    assert!(
        red.is_empty(),
        "hand-authored replay: {} red case(s): {red:?}",
        red.len()
    );
}

fn replay_case(case: HandAuthoredCase, checkpoints: &Checkpoints, locks: &Locks) {
    // A pinned `initial_state` rebuilds the failing draw's oracle exactly;
    // without one the base is `HOLON_PBT_FORCE_FULL=1`'s `full_headless`
    // wiring, so `init_test` boots the full Turso + frontend SUT.
    let provenance = format!("hand-authored regression case {:?}", case.name);
    let initial_state = match case.initial_state {
        Some(pinned) => pinned.into_reference_state(&provenance),
        None => wide_e2e_ref(),
    };
    // `wide_e2e_ref_for` is NOT pure: `HOLON_FOLDER_COMPANION_SEED` makes it
    // seed extra blocks and `PBT_MUTABLE_TEXT` changes the reference's
    // editor semantics, so a byte-identical `initial_state` still yields
    // different starting states under different flags. `environment` is
    // where a capture records them; compare, never assume.
    if let Some(report) = case
        .environment
        .mismatch_report(Some(&initial_state.harness.wiring))
    {
        panic!(
            "{provenance}: recorded capture environment does not match this replay — the \
             starting state differs from the one this case pins:\n{report}"
        );
    }
    replay::<ComposedSut<WideE2E>>(initial_state, case.transitions, checkpoints, locks);
}

/// PARKED pin — echo-loop `BlockToPage` embedded-page child render-leak
/// (BugFunnel rows 142 file-authority-echo / 81 echo-loop-blocked).
///
/// Deterministic repro (no RNG): create `origin` + `child` under the boot
/// focus page, `Indent` child under origin, toggle it DOING, then
/// `BlockToPage(origin)`. The origin becomes a `[[P]]` link, a new page P is
/// minted, and `child` re-homes under P — in `block_raw`, the matview, Loro,
/// the org file AND the watch rowset (all block-comparison invariants stay
/// green). The DIVERGENCE is render-only: P renders as a COLLAPSED embedded
/// page in the main panel, yet `child` leaks into the main-panel widget tree,
/// tripping `inv-embedded-page-collapsed-lazy`
/// (`bodies/embedded_page_collapsed_lazy.rs:176`, reading
/// `SutRenderer::widget_tree_snapshot`).
///
/// Why parked, not fixed:
///  * The 2026-07-23 triage's PRIME SUSPECT — the ancestor writeback leaving
///    the moved subtree in its file so a re-ingest DOUBLE-HOMES `child` and
///    reverts its `block_raw` parent — is REFUTED by this keystone: appending
///    `SimulateRestart` (a real FileSyncController re-ingest tick) leaves every
///    block/org/watch invariant green; the ONLY residual failure is still the
///    render leak. Writeback prunes correctly; persistence never reverts.
///  * `inv-embedded-page-collapsed-lazy` is a KNOWN PARKED keystone invariant.
///    Whether the fix belongs in the SUT (the embedded-page main-panel assembly
///    must prune a freshly-converted collapsed page's re-homed children) or in
///    the oracle (a just-converted page may legitimately render its children
///    until the next lazy tick) is a RULING for Martin — it touches the
///    embedded-page render machinery broadly, so it is not a bounded fix.
///
/// One flag-flip from enforcement: delete `#[ignore]` and this goes RED for the
/// right reason (identical signature to the keystone sweep panic
/// `harness.rs:686` on `inv-embedded-page-collapsed-lazy`).
#[test]
fn echo_loop_block_to_page_child_render_leak_parked() {
    // Replayed through the EXACT keystone harness the JSONL cases use — the
    // serde shape is the canonical `Fixture<E2ETransition>`, so this stays a
    // one-line-flip from a live JSONL regression once the ruling lands.
    let line = r#"{"name": "block-to-page-child-render-leak", "description": "echo-loop (BugFunnel 142/81): re-homed child leaks into the collapsed embedded page's main panel", "transitions": [{"CreateBlockUnderFocus": {"content": "origin", "id": "block:echoorigin"}}, {"CreateBlockUnderFocus": {"content": "child", "id": "block:echochild"}}, {"Indent": {"block_id": "block:echochild"}}, {"ToggleState": {"block_id": "block:echochild", "new_state": "DOING"}}, {"BlockToPage": {"origin_id": "block:echoorigin"}}]}"#;
    let case: HandAuthoredCase =
        serde_json::from_str(line).expect("parked echo-loop case must parse");
    let config = Config {
        verbose: 1,
        ..Config::default()
    };
    let initial_state = wide_e2e_ref();
    ComposedSut::<WideE2E>::test_sequential(config, initial_state, case.transitions, None);
}

/// A line without `initial_state` keeps the historical meaning: replay over the
/// fixed `wide_e2e_ref()` base.
#[test]
fn absent_initial_state_parses_as_none() {
    let line = r#"{"name": "n", "transitions": []}"#;
    let case = parse_case(std::path::Path::new("<inline>"), 1, line);
    assert_eq!(case.initial_state, None);
}

/// The capture is lossless: every wiring manifest survives
/// serialize→parse→compare, and the parsed manifest rebuilds a `ReferenceState`
/// carrying exactly that wiring.
#[test]
fn pinned_initial_state_round_trips_and_rebuilds_the_draw() {
    let wiring = holon_pbt_core::wiring_from_exact_spec("Turso;;");
    let line = serde_json::to_string(&serde_json::json!({
        "name": "n",
        "transitions": [],
        "initial_state": {"wiring": wiring},
    }))
    .expect("case serializes");
    let case = parse_case(std::path::Path::new("<inline>"), 1, &line);
    let pinned = case.initial_state.expect("initial_state parsed");
    assert_eq!(pinned.wiring, wiring);
    let rebuilt = pinned.into_reference_state("<inline>");
    assert_eq!(rebuilt.harness.wiring, wiring);
}

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

/// A case recorded under an env flag that mutates the starting state must not
/// replay silently without it.
#[test]
fn recorded_env_flag_mismatch_is_reported() {
    let mut env = holon_pbt_core::fixture::CaptureEnvironment::default();
    env.env_flags
        .insert("HOLON_FOLDER_COMPANION_SEED".to_string(), "1".to_string());
    let report = env.mismatch_report(None);
    assert!(
        report.is_some_and(|r| r.contains("env flags differ")),
        "a recorded flag absent from the live env must be reported"
    );
}

thread_local! {
    /// Transitions applied when each sweep of [`CountingSut`] ran.
    static SWEPT: std::cell::RefCell<Vec<usize>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Counts transitions applied; stands in for the keystone reference.
struct CountingRef;

impl proptest_state_machine::ReferenceStateMachine for CountingRef {
    type State = usize;
    type Transition = ();

    fn init_state() -> proptest::strategy::BoxedStrategy<usize> {
        proptest::strategy::Strategy::boxed(proptest::strategy::Just(0))
    }

    fn transitions(_: &usize) -> proptest::strategy::BoxedStrategy<()> {
        proptest::strategy::Strategy::boxed(proptest::strategy::Just(()))
    }

    fn apply(applied: usize, _: &()) -> usize {
        applied + 1
    }
}

/// Records every invariant sweep into [`SWEPT`].
struct CountingSut;

impl StateMachineTest for CountingSut {
    type SystemUnderTest = ();
    type Reference = CountingRef;

    fn init_test(_: &usize) {}

    fn apply(sut: (), _: &usize, _: ()) {
        sut
    }

    fn check_invariants(_: &(), applied: &usize) {
        SWEPT.with(|swept| swept.borrow_mut().push(*applied));
    }
}

/// Engages the inline cases' lock at every checkpoint.
impl LockedReplay for CountingSut {
    fn check_locked(sut: &(), applied: &usize, _: &CheckScope) -> BTreeSet<&'static str> {
        Self::check_invariants(sut, applied);
        BTreeSet::from(["inv-x"])
    }

    fn teardown_locked(_: (), _: usize, _: &CheckScope) -> BTreeSet<&'static str> {
        BTreeSet::new()
    }
}

fn four_transition_case(extra: &str) -> String {
    let create = r#"{"CreateBlockUnderFocus": {"content": "c", "id": "block:c"}}"#;
    format!(
        r#"{{"name": "n", "locks": ["inv-x"], "transitions": [{create}, {create}, {create}, {create}]{extra}}}"#
    )
}

fn sweeps_of_replay(line: &str) -> Vec<usize> {
    let ReplayCase {
        case,
        checkpoints,
        locks,
        ..
    } = parse_replay_case(std::path::Path::new("<inline>"), 1, line);
    SWEPT.with(|swept| swept.borrow_mut().clear());
    replay::<CountingSut>(0, vec![(); case.transitions.len()], &checkpoints, &locks);
    SWEPT.with(|swept| swept.take())
}

#[test]
fn replay_sweeps_around_the_last_transition_only() {
    assert_eq!(sweeps_of_replay(&four_transition_case("")), vec![3, 4]);
}

#[test]
fn replay_also_sweeps_around_each_named_transition() {
    assert_eq!(
        sweeps_of_replay(&four_transition_case(r#", "also_check_after": [1]"#)),
        vec![0, 1, 3, 4]
    );
}

#[test]
fn replay_of_a_case_without_transitions_sweeps_once_after_boot() {
    assert_eq!(
        sweeps_of_replay(r#"{"name": "n", "locks": ["inv-x"], "transitions": []}"#),
        vec![0]
    );
}

/// The drawn keystone runs `test_sequential`, which sweeps after boot and after
/// every transition.
#[test]
fn drawn_run_sweeps_after_boot_and_every_transition() {
    SWEPT.with(|swept| swept.borrow_mut().clear());
    CountingSut::test_sequential(Config::default(), 0, vec![(); 4], None);
    assert_eq!(SWEPT.with(|swept| swept.take()), vec![0, 1, 2, 3, 4]);
}

thread_local! {
    static STARTED_SWEPT: std::cell::RefCell<Vec<(usize, bool)>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

/// Drives the production budget lifecycle and records, per sweep, whether the
/// window `inv-sql-budget` would judge started from a swept state.
struct BudgetWindowSut;

impl StateMachineTest for BudgetWindowSut {
    type SystemUnderTest = ComposedSpanMetrics;
    type Reference = CountingRef;

    fn init_test(_: &usize) -> ComposedSpanMetrics {
        ComposedSpanMetrics::new()
    }

    fn apply(sut: ComposedSpanMetrics, _: &usize, _: ()) -> ComposedSpanMetrics {
        sut.note_transition_start(&E2ETransition::UndoLastMutation(UndoLastMutation));
        sut
    }

    fn check_invariants(sut: &ComposedSpanMetrics, applied: &usize) {
        sut.freeze_for_check(&ReferenceState::new(
            holon_pbt_core::Wiring::full(),
            std::sync::Arc::new(holon_frontend::render_interpreter::RenderInterpreter::new()),
        ));
        STARTED_SWEPT.with(|log| {
            log.borrow_mut()
                .push((*applied, sut.window_started_swept()))
        });
    }
}

impl LockedReplay for BudgetWindowSut {
    fn check_locked(
        sut: &ComposedSpanMetrics,
        applied: &usize,
        _: &CheckScope,
    ) -> BTreeSet<&'static str> {
        Self::check_invariants(sut, applied);
        BTreeSet::from(["inv-x"])
    }

    fn teardown_locked(_: ComposedSpanMetrics, _: usize, _: &CheckScope) -> BTreeSet<&'static str> {
        BTreeSet::new()
    }
}

/// `inv-sql-budget` skips exactly the windows that started unswept, so every
/// judged transition must start swept and only the pre-sweeps may not.
#[test]
fn judged_windows_start_swept_and_only_pre_sweeps_do_not() {
    let line = format!(
        r#"{{"name": "n", "locks": ["inv-x"], "transitions": [{0}, {0}, {0}, {0}, {0}, {0}], "also_check_after": [2]}}"#,
        r#"{"CreateBlockUnderFocus": {"content": "c", "id": "block:c"}}"#
    );
    let ReplayCase {
        checkpoints, locks, ..
    } = parse_replay_case(std::path::Path::new("<inline>"), 1, &line);
    STARTED_SWEPT.with(|log| log.borrow_mut().clear());
    replay::<BudgetWindowSut>(0, vec![(); 6], &checkpoints, &locks);
    assert_eq!(
        STARTED_SWEPT.with(|log| log.take()),
        vec![(1, false), (2, true), (5, false), (6, true)]
    );
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
    replay::<VacuousSut>(0, Vec::new(), &checkpoints, &locks);
}

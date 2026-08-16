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
//! keystone harness (`ComposedSut::<WideE2E>::test_sequential`): the same SUT
//! boot, the same production transition `apply`, and the same composed
//! invariant catalog the random keystone runs. The only thing that differs
//! from the random keystone is the source of the `(initial_state,
//! transitions)` pair — here both halves come from the JSONL instead of a
//! `proptest` draw.
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
//!   The driver stops at the first red, so without this a probe case is masked
//!   by any earlier failing case.
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

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::hand_authored::CapturedInitialState;
use holon_integration_tests::pbt::hand_authored::HandAuthoredCase;
use holon_integration_tests::pbt::hand_authored::load_cases;
use holon_integration_tests::pbt::hand_authored::parse_case;
use holon_integration_tests::pbt::hand_authored::sidecar_path;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

/// Replay every hand-authored keystone regression through the production
/// harness. A case that reproduces a live defect (or breaks an invariant) fails
/// here exactly as the random keystone would — and becomes a permanent lock:
/// green once the defect is fixed.
#[test]
fn hand_authored_keystone_regressions() {
    let cases = load_cases();
    assert!(
        !cases.is_empty(),
        "no hand-authored regression cases found in {:?} — the sidecar must not be empty",
        sidecar_path()
    );

    // Direct replay: `cases`/`max_shrink_iters` are irrelevant (we apply one
    // fixed sequence per case, never generate or shrink). `verbose >= 1` makes
    // `test_sequential` disclose each transition it applies on stderr.
    let config = Config {
        verbose: 1,
        ..Config::default()
    };

    for case in cases {
        eprintln!(
            "[hand-authored regression] running case {:?} ({} transitions)",
            case.name,
            case.transitions.len()
        );
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
        ComposedSut::<WideE2E>::test_sequential(
            config.clone(),
            initial_state,
            case.transitions,
            None,
        );
        eprintln!("[hand-authored regression] PASSED case {:?}", case.name);
    }
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

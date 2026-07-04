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

use std::path::PathBuf;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref_for;
use holon_integration_tests::pbt::reference_state::ReferenceState;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
use holon_pbt_core::wiring::Wiring;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;
use serde::Deserialize;
use serde::Serialize;

/// Sidecar of hand-authored keystone regressions, relative to the crate root.
/// One JSON object per line (JSONL); `#`-prefixed and blank lines are ignored.
/// Overridable via `HOLON_HAND_AUTHORED_SIDECAR`.
const DEFAULT_SIDECAR: &str = "hand-authored-regressions/keystone.jsonl";

/// Env var holding an alternative sidecar path (absolute, or relative to the
/// crate root).
const SIDECAR_ENV: &str = "HOLON_HAND_AUTHORED_SIDECAR";

/// Env var holding a comma-separated list of case names to run in isolation.
const CASE_FILTER_ENV: &str = "HOLON_HAND_AUTHORED_CASE";

/// Env var holding a comma-separated list of case names to QUARANTINE (skip),
/// so a known-bad case can be excluded via env instead of a code edit that
/// could be forgotten when the underlying fix lands.
const SKIP_FILTER_ENV: &str = "HOLON_HAND_AUTHORED_SKIP";

/// The generated half of a keystone case's starting point.
///
/// One field, because `WideE2EMachine::init_state` draws one value. Serde is
/// strict on purpose: `deny_unknown_fields` here plus the round-trip check in
/// [`parse_case`] means a renamed or mistyped field is a loud parse failure,
/// never a silently-defaulted replay of a DIFFERENT starting state than the
/// one that failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CapturedInitialState {
    /// The manifest drawn by `init_state` — copy it verbatim out of the
    /// failing case's `[pbt-telemetry]` line (`.wiring`) or its
    /// `[wide-e2e wiring] drawn:` line.
    wiring: Wiring,
}

impl CapturedInitialState {
    /// Rebuild the exact `ReferenceState` the failing draw started from.
    fn into_reference_state(self, provenance: &str) -> ReferenceState {
        if let Err(e) = self.wiring.validate() {
            panic!(
                "{provenance}: initial_state carries an INVALID wiring manifest {:?}: {e} — \
                 `init_state` only ever draws valid manifests, so this case could not have come \
                 from a real run",
                self.wiring
            );
        }
        wide_e2e_ref_for(&self.wiring)
    }
}

/// One hand-authored regression: a human-written twin of a persisted keystone
/// failure, but with the transition sequence spelled out concretely instead of
/// hidden behind an RNG seed.
///
/// This is the canonical value-level [`Fixture`] format (`holon-pbt-core`), the
/// SINGLE serialization shape for hand-authored regression cases — not a
/// bespoke parallel struct. A case pins `name` / `description` / `transitions`
/// and MAY pin `initial_state`; a line that omits `initial_state` replays over
/// [`wide_e2e_ref`].
///
/// Upstream convergence: proptest PR #653 (value persistence) is the eventual
/// single-format home for this per the 2026-06-25 decision; Holon pins upstream
/// v1.10.0 (seed-only) until it lands. Do not fold this onto the seed file
/// before then — a seed regression is not hand-authorable (see the module doc
/// above).
type HandAuthoredCase = Fixture<E2ETransition, Option<CapturedInitialState>>;

/// Every top-level key a sidecar line may carry — the `Fixture` field set.
///
/// `Fixture` itself is NOT `deny_unknown_fields` (it is shared with the gated
/// slices' `.fixtures/` corpora), so serde silently drops a top-level key it
/// does not recognize. That makes a typo in the KEY the one authoring mistake
/// the inner schema guards cannot see: `"initail_state"` parses fine, leaves
/// `initial_state = None`, and replays over `full_headless` instead of the
/// pinned wiring — a green that means nothing. [`parse_case`] rejects any key
/// outside this list.
const KNOWN_CASE_KEYS: &[&str] = &[
    "name",
    "description",
    "environment",
    "initial_state",
    "transitions",
];

fn sidecar_path() -> PathBuf {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match std::env::var(SIDECAR_ENV) {
        Ok(override_path) => crate_root.join(override_path),
        Err(std::env::VarError::NotPresent) => crate_root.join(DEFAULT_SIDECAR),
        Err(e) => panic!("{SIDECAR_ENV} is set but unreadable: {e}"),
    }
}

/// Case names to run, from [`CASE_FILTER_ENV`]; `None` means "run all".
fn case_filter() -> Option<Vec<String>> {
    match std::env::var(CASE_FILTER_ENV) {
        Ok(spec) => {
            let names: Vec<String> = spec
                .split(',')
                .map(str::trim)
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect();
            assert!(
                !names.is_empty(),
                "{CASE_FILTER_ENV} is set to {spec:?} but names no case"
            );
            Some(names)
        }
        Err(std::env::VarError::NotPresent) => None,
        Err(e) => panic!("{CASE_FILTER_ENV} is set but unreadable: {e}"),
    }
}

/// Case names to quarantine, from [`SKIP_FILTER_ENV`]; an empty vec means "skip
/// nothing". Same parse plumbing as [`case_filter`], mirrored deliberately.
fn skip_filter() -> Vec<String> {
    match std::env::var(SKIP_FILTER_ENV) {
        Ok(spec) => spec
            .split(',')
            .map(str::trim)
            .filter(|n| !n.is_empty())
            .map(str::to_string)
            .collect(),
        Err(std::env::VarError::NotPresent) => Vec::new(),
        Err(e) => panic!("{SKIP_FILTER_ENV} is set but unreadable: {e}"),
    }
}

/// Parse ONE sidecar line into a case, failing loud on anything short of a
/// total, lossless read.
///
/// The round-trip check is the schema-mismatch guard: serde silently tolerates
/// unknown fields nested inside `Wiring`'s axes, so re-serializing the parsed
/// `initial_state` and comparing it against the raw JSON sub-value proves that
/// NOTHING in the authored bytes was dropped. A case that replays a subtly
/// different starting state than the one that failed is worse than no case at
/// all, so this is a panic, not a warning.
fn parse_case(path: &std::path::Path, lineno: usize, line: &str) -> HandAuthoredCase {
    let provenance = format!("hand-authored regression {path:?}:{lineno}");
    let case = serde_json::from_str::<HandAuthoredCase>(line).unwrap_or_else(|e| {
        panic!("{provenance} failed to parse: {e}\n  line: {line}");
    });
    let raw: serde_json::Value = serde_json::from_str(line)
        .unwrap_or_else(|e| panic!("{provenance} is not valid JSON: {e}\n  line: {line}"));
    let raw_object = raw
        .as_object()
        .unwrap_or_else(|| panic!("{provenance} is not a JSON object\n  line: {line}"));
    for key in raw_object.keys() {
        assert!(
            KNOWN_CASE_KEYS.contains(&key.as_str()),
            "{provenance}: unknown top-level key {key:?} — a mistyped key is SILENTLY DROPPED by \
             serde, so this case would replay something other than what it names. Known keys: \
             {KNOWN_CASE_KEYS:?}\n  line: {line}"
        );
    }
    let raw_initial = raw.get("initial_state").unwrap_or(&serde_json::Value::Null);
    let reserialized = serde_json::to_value(&case.initial_state)
        .unwrap_or_else(|e| panic!("{provenance}: parsed initial_state does not serialize: {e}"));
    assert_eq!(
        raw_initial, &reserialized,
        "{provenance}: `initial_state` does not round-trip — the authored JSON and the parsed \
         value differ, so replaying this case would start from a DIFFERENT state than the run it \
         pins"
    );
    case
}

/// Parse the sidecar, failing loud on the FIRST malformed line (file:line +
/// the raw line + serde's message). No `.ok()` / `_ => skip` — an unparseable
/// hand-authored case is a bug in the case, surfaced immediately.
fn load_cases() -> Vec<HandAuthoredCase> {
    let path = sidecar_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("hand-authored regression sidecar {path:?} is unreadable: {e}"));
    let cases: Vec<HandAuthoredCase> = raw
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(lineno, line)| parse_case(&path, lineno, line))
        .collect();

    let known: Vec<&str> = cases.iter().map(|c| c.name.as_str()).collect();

    // Quarantine (skip) filter. Validated against ALL parsed cases up front so a
    // typo in the skip list is a hard error, never a silent quarantine-nothing.
    let skip = skip_filter();
    for name in &skip {
        assert!(
            known.contains(&name.as_str()),
            "{SKIP_FILTER_ENV} names case {name:?} to skip, absent from {path:?}; \
             known cases: {known:?}"
        );
    }

    let cases: Vec<HandAuthoredCase> = match case_filter() {
        None => cases,
        Some(wanted) => {
            for name in &wanted {
                assert!(
                    known.contains(&name.as_str()),
                    "{CASE_FILTER_ENV} names case {name:?}, absent from {path:?}; \
                     known cases: {known:?}"
                );
            }
            cases
                .into_iter()
                .filter(|c| wanted.contains(&c.name))
                .collect()
        }
    };

    if skip.is_empty() {
        return cases;
    }
    let mut skipped = 0usize;
    let kept: Vec<HandAuthoredCase> = cases
        .into_iter()
        .filter(|c| {
            if skip.contains(&c.name) {
                eprintln!(
                    "[hand-authored regression] SKIPPED case {:?} via {SKIP_FILTER_ENV} \
                     (known issue \u{2014} see docs/Testing/BugFunnel.md)",
                    c.name
                );
                skipped += 1;
                false
            } else {
                true
            }
        })
        .collect();
    eprintln!(
        "[hand-authored regression] {skipped} case(s) SKIPPED via {SKIP_FILTER_ENV} \
         \u{2014} coverage is NOT full (see docs/Testing/BugFunnel.md)"
    );
    kept
}

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
/// [`KNOWN_CASE_KEYS`] allowlist catches it.
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

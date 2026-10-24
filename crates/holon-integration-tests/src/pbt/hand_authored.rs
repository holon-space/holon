//! Hand-authored keystone regression cases: the sidecar schema and its loader.
//!
//! Lives in the library, not in the test that replays them, because the corpus
//! has more than one consumer: `tests/hand_authored_regressions.rs` drives each
//! case through the headless `ComposedSut`, and the web arm
//! (`crate::web_arm`) replays the DOM-drivable subset in a real browser. Per
//! the standing directive, a dedicated arm REUSES the keystone's structures —
//! promotion is a move, not a rewrite — so the parse, the schema guards and the
//! env filters are shared and a case can never mean two different things to two
//! consumers.
//!
//! See `docs/Testing/HandAuthoredRegressions.md` for the authoring procedure.

use std::collections::BTreeSet;
use std::path::PathBuf;

use holon_pbt_core::fixture::Fixture;
use holon_pbt_core::wiring::Wiring;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;
use serde::Deserialize;
use serde::Serialize;

use crate::pbt::composed::harness::ComposedSut;
use crate::pbt::composed::wide_e2e::WideE2E;
use crate::pbt::composed::wide_e2e::wide_e2e_ref_for;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::transitions::E2ETransition;

/// Sidecar of hand-authored keystone regressions, relative to the crate root.
/// One JSON object per line (JSONL); `#`-prefixed and blank lines are ignored.
/// Overridable via `HOLON_HAND_AUTHORED_SIDECAR`.
pub const DEFAULT_SIDECAR: &str = "hand-authored-regressions/keystone.jsonl";

/// Env var holding an alternative sidecar path (absolute, or relative to the
/// crate root).
pub const SIDECAR_ENV: &str = "HOLON_HAND_AUTHORED_SIDECAR";

/// Env var holding a comma-separated list of case names to run in isolation.
pub const CASE_FILTER_ENV: &str = "HOLON_HAND_AUTHORED_CASE";

/// Env var holding a comma-separated list of case names to QUARANTINE (skip),
/// so a known-bad case can be excluded via env instead of a code edit that
/// could be forgotten when the underlying fix lands.
pub const SKIP_FILTER_ENV: &str = "HOLON_HAND_AUTHORED_SKIP";

/// The generated half of a keystone case's starting point.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapturedInitialState {
    /// The manifest drawn by `init_state` — copy it verbatim out of the
    /// failing case's `[pbt-telemetry]` line (`.wiring`) or its
    /// `[wide-e2e wiring] drawn:` line.
    pub wiring: Wiring,
}

impl CapturedInitialState {
    /// Rebuild the exact `ReferenceState` the failing draw started from.
    pub fn into_reference_state(self, provenance: &str) -> ReferenceState {
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
pub type HandAuthoredCase = Fixture<E2ETransition, Option<CapturedInitialState>>;

/// Every top-level key a sidecar line may carry — the `Fixture` field set.
///
/// `Fixture` itself is NOT `deny_unknown_fields` (it is shared with the gated
/// slices' `.fixtures/` corpora), so serde silently drops a top-level key it
/// does not recognize. That makes a typo in the KEY the one authoring mistake
/// the inner schema guards cannot see: `"initail_state"` parses fine, leaves
/// `initial_state = None`, and replays over `full_headless` instead of the
/// pinned wiring — a green that means nothing. [`parse_case`] rejects any key
/// outside this list.
pub const KNOWN_CASE_KEYS: &[&str] = &[
    "name",
    "description",
    "environment",
    "initial_state",
    "transitions",
    ALSO_CHECK_AFTER_KEY,
    LOCKS_KEY,
];

/// The replay-only key naming extra checkpoints; see [`Checkpoints`].
pub const ALSO_CHECK_AFTER_KEY: &str = "also_check_after";

/// The replay-only key naming the invariants a case locks; see [`Locks`].
pub const LOCKS_KEY: &str = "locks";

/// Judged by every case on top of its [`Locks`]; the reason for each member is
/// in `docs/Testing/HandAuthoredRegressions.md`.
pub const LOCK_FLOOR: &[&str] = &[
    "inv-no-observed-errors",
    "inv-no-errors",
    "inv-no-parent-cycles",
    "inv-no-orphan-blocks",
];

/// Invariants that judge what a transition costs. Their budgets are measured
/// after a whole-catalog sweep, whose reads warm the views the transition would
/// otherwise build, so a case that locks one sweeps the whole catalog.
pub const COST_LOCKS: &[&str] = &["inv-sql-budget"];

/// What one sweep of [`replay`] runs and judges.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckScope {
    pub judged: BTreeSet<String>,
    /// Runs every catalog invariant but judges only `judged`.
    pub runs_whole_catalog: bool,
}

/// The catalog invariant ids a case's bug breaks: [`replay`] judges these plus
/// [`LOCK_FLOOR`] and nothing else, and each must produce a non-`Skipped`
/// verdict at one checkpoint at least.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Locks(BTreeSet<String>);

impl Locks {
    pub fn ids(&self) -> &BTreeSet<String> {
        &self.0
    }

    /// Judges the locks plus [`LOCK_FLOOR`].
    pub fn scope(&self) -> CheckScope {
        CheckScope {
            judged: self
                .0
                .iter()
                .cloned()
                .chain(LOCK_FLOOR.iter().map(|id| id.to_string()))
                .collect(),
            runs_whole_catalog: COST_LOCKS.iter().any(|id| self.0.contains(*id)),
        }
    }

    fn parse(raw: Option<&serde_json::Value>, provenance: &str) -> Self {
        let raw = raw.unwrap_or_else(|| {
            panic!(
                "{provenance} has no `{LOCKS_KEY}` field: name the catalog invariant id(s) its \
                 bug breaks (docs/Testing/HandAuthoredRegressions.md)"
            )
        });
        let named = raw.as_array().unwrap_or_else(|| {
            panic!("{provenance}: `{LOCKS_KEY}` must be an array of invariant ids, got {raw}")
        });
        assert!(
            !named.is_empty(),
            "{provenance}: `{LOCKS_KEY}` is empty; a case locks one invariant at least"
        );
        let mut ids = BTreeSet::new();
        for value in named {
            let id = value.as_str().unwrap_or_else(|| {
                panic!("{provenance}: `{LOCKS_KEY}` holds {value}, not an invariant id")
            });
            assert!(
                ids.insert(id.to_string()),
                "{provenance}: `{LOCKS_KEY}` names {id:?} twice"
            );
        }
        Self(ids)
    }
}

/// The transitions [`replay`] judges, as the 1-based numbers `Applying
/// transition k/N` prints: always the last one, plus any the case names in
/// `also_check_after` because its bug shows mid-sequence and a later transition
/// heals it. The check also runs just BEFORE each judged transition (`0` is
/// after boot): its sweep warms views a drawn run's sweeps warm, so the judged
/// transition's SQL budget window measures the same work as in a drawn run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkpoints(BTreeSet<usize>);

impl Checkpoints {
    pub fn after(&self, transition: usize) -> bool {
        self.0.contains(&transition) || self.0.contains(&(transition + 1))
    }

    fn parse(raw: Option<&serde_json::Value>, transitions: usize, provenance: &str) -> Self {
        let mut points = BTreeSet::from([transitions]);
        let Some(raw) = raw else {
            return Self(points);
        };
        let named = raw.as_array().unwrap_or_else(|| {
            panic!("{provenance}: `{ALSO_CHECK_AFTER_KEY}` must be an array of transition numbers, got {raw}")
        });
        assert!(
            !named.is_empty(),
            "{provenance}: `{ALSO_CHECK_AFTER_KEY}` is empty; omit it to check only the last transition"
        );
        for value in named {
            let k = value.as_u64().unwrap_or_else(|| {
                panic!(
                    "{provenance}: `{ALSO_CHECK_AFTER_KEY}` holds {value}, not a transition number"
                )
            }) as usize;
            assert!(
                (1..transitions).contains(&k),
                "{provenance}: `{ALSO_CHECK_AFTER_KEY}` names transition {k}, out of range 1..{} \
                 (1-based; the last transition {transitions} is always checked)",
                transitions.saturating_sub(1)
            );
            assert!(
                points.insert(k),
                "{provenance}: `{ALSO_CHECK_AFTER_KEY}` names transition {k} twice"
            );
        }
        Self(points)
    }
}

/// A sidecar case together with where and what its replay judges.
pub struct ReplayCase {
    pub case: HandAuthoredCase,
    pub checkpoints: Checkpoints,
    pub locks: Locks,
}

/// A [`StateMachineTest`] whose check can be narrowed to part of the catalog.
pub trait LockedReplay: StateMachineTest {
    /// Judges `scope`; returns the judged ids with a non-`Skipped` verdict.
    fn check_locked(
        sut: &Self::SystemUnderTest,
        ref_state: &<Self::Reference as ReferenceStateMachine>::State,
        scope: &CheckScope,
    ) -> BTreeSet<&'static str>;

    /// [`StateMachineTest::teardown`] with [`Self::check_locked`] as its check.
    fn teardown_locked(
        sut: Self::SystemUnderTest,
        ref_state: <Self::Reference as ReferenceStateMachine>::State,
        scope: &CheckScope,
    ) -> BTreeSet<&'static str>;
}

impl LockedReplay for ComposedSut<WideE2E> {
    fn check_locked(
        sut: &Self,
        ref_state: &ReferenceState,
        scope: &CheckScope,
    ) -> BTreeSet<&'static str> {
        sut.check_scoped(ref_state, scope)
    }

    fn teardown_locked(
        sut: Self,
        ref_state: ReferenceState,
        scope: &CheckScope,
    ) -> BTreeSet<&'static str> {
        sut.teardown_scoped(ref_state, scope)
    }
}

/// The keystone's `test_sequential`, except that it judges only the case's
/// [`Locks`] plus [`LOCK_FLOOR`], and only around the transitions in
/// `checkpoints`: a hand-authored case locks one known bug, and the random
/// keystone judges the rest.
pub fn replay<M: LockedReplay>(
    mut ref_state: <M::Reference as ReferenceStateMachine>::State,
    transitions: Vec<<M::Reference as ReferenceStateMachine>::Transition>,
    checkpoints: &Checkpoints,
    locks: &Locks,
) {
    let scope = locks.scope();
    let mut engaged = BTreeSet::new();
    let total = transitions.len();
    let mut sut = M::init_test(&ref_state);
    if checkpoints.after(0) {
        engaged.extend(M::check_locked(&sut, &ref_state, &scope));
    }
    for (ix, transition) in transitions.into_iter().enumerate() {
        eprintln!("\nApplying transition {}/{total}: {transition:?}", ix + 1);
        ref_state = M::Reference::apply(ref_state, &transition);
        sut = M::apply(sut, &ref_state, transition);
        if checkpoints.after(ix + 1) {
            engaged.extend(M::check_locked(&sut, &ref_state, &scope));
        }
    }
    engaged.extend(M::teardown_locked(sut, ref_state, &scope));
    let unengaged: Vec<&String> = locks
        .ids()
        .iter()
        .filter(|id| !engaged.contains(id.as_str()))
        .collect();
    assert!(
        unengaged.is_empty(),
        "locked invariant(s) {unengaged:?} were `Skipped` at every checkpoint: the case never \
         reaches the state they judge, so it locks nothing (engaged: {engaged:?})"
    );
}

pub fn sidecar_path() -> PathBuf {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    match std::env::var(SIDECAR_ENV) {
        Ok(override_path) => crate_root.join(override_path),
        Err(std::env::VarError::NotPresent) => crate_root.join(DEFAULT_SIDECAR),
        Err(e) => panic!("{SIDECAR_ENV} is set but unreadable: {e}"),
    }
}

/// Case names to run, from [`CASE_FILTER_ENV`]; `None` means "run all".
pub fn case_filter() -> Option<Vec<String>> {
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
pub fn skip_filter() -> Vec<String> {
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
pub fn parse_case(path: &std::path::Path, lineno: usize, line: &str) -> HandAuthoredCase {
    parse_raw(path, lineno, line).0
}

/// [`parse_case`] plus the case's [`Checkpoints`] and [`Locks`].
pub fn parse_replay_case(path: &std::path::Path, lineno: usize, line: &str) -> ReplayCase {
    let (case, raw, provenance) = parse_raw(path, lineno, line);
    let provenance = format!("{provenance} case {:?}", case.name);
    let checkpoints = Checkpoints::parse(
        raw.get(ALSO_CHECK_AFTER_KEY),
        case.transitions.len(),
        &provenance,
    );
    let locks = Locks::parse(raw.get(LOCKS_KEY), &provenance);
    ReplayCase {
        case,
        checkpoints,
        locks,
    }
}

fn parse_raw(
    path: &std::path::Path,
    lineno: usize,
    line: &str,
) -> (HandAuthoredCase, serde_json::Value, String) {
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
    (case, raw, provenance)
}

/// Parse the sidecar, failing loud on the FIRST malformed line (file:line +
/// the raw line + serde's message). No `.ok()` / `_ => skip` — an unparseable
/// hand-authored case is a bug in the case, surfaced immediately.
pub fn load_cases() -> Vec<HandAuthoredCase> {
    load_replay_cases().into_iter().map(|c| c.case).collect()
}

/// [`load_cases`] plus each case's [`Checkpoints`].
pub fn load_replay_cases() -> Vec<ReplayCase> {
    let path = sidecar_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("hand-authored regression sidecar {path:?} is unreadable: {e}"));
    let cases: Vec<ReplayCase> = raw
        .lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(lineno, line)| parse_replay_case(&path, lineno, line))
        .collect();

    let known: Vec<&str> = cases.iter().map(|c| c.case.name.as_str()).collect();

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

    let cases: Vec<ReplayCase> = match case_filter() {
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
                .filter(|c| wanted.contains(&c.case.name))
                .collect()
        }
    };

    if skip.is_empty() {
        return cases;
    }
    let mut skipped = 0usize;
    let kept: Vec<ReplayCase> = cases
        .into_iter()
        .filter(|c| {
            if skip.contains(&c.case.name) {
                eprintln!(
                    "[hand-authored regression] SKIPPED case {:?} via {SKIP_FILTER_ENV} \
                     (known issue \u{2014} see docs/Testing/BugFunnel.md)",
                    c.case.name
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

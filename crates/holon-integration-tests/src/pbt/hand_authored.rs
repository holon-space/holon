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

use std::path::PathBuf;

use holon_pbt_core::fixture::Fixture;
use holon_pbt_core::wiring::Wiring;
use serde::Deserialize;
use serde::Serialize;

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
];

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
pub fn load_cases() -> Vec<HandAuthoredCase> {
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

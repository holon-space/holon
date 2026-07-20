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
//! hand-authored cases — [`SIDECAR`], one JSON object per line — where each
//! case is a **serialized `Vec<E2ETransition>`** (the production transition
//! enum derives `serde`). It then drives each case through the EXACT keystone
//! harness (`ComposedSut::<WideE2E>::test_sequential`): the same SUT boot, the
//! same production transition `apply`, and the same composed invariant catalog
//! the random keystone runs. The only thing that differs from the random
//! keystone is the source of the `(initial_state, transitions)` pair — here it
//! is a fixed base oracle ([`wide_e2e_ref`], the `full_headless` wiring,
//! identical to `HOLON_PBT_FORCE_FULL=1`) plus the hand-authored transition
//! list, instead of a `proptest` draw.
//!
//! Fail LOUD (parse-don't-validate): an unreadable sidecar, a malformed line,
//! or an unknown transition variant panics with the offending file:line — never
//! a silent skip. A case that trips a production defect or an invariant fails
//! the test exactly as it would in the random keystone.
//!
//! See `docs/Testing/HandAuthoredRegressions.md` for the authoring procedure
//! and the limits of what a hand-authored regression can express.

use std::path::PathBuf;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
use proptest::test_runner::Config;
use proptest_state_machine::StateMachineTest;

/// Sidecar of hand-authored keystone regressions, relative to the crate root.
/// One JSON object per line (JSONL); `#`-prefixed and blank lines are ignored.
const SIDECAR: &str = "hand-authored-regressions/keystone.jsonl";

/// One hand-authored regression: a human-written twin of a persisted keystone
/// failure, but with the transition sequence spelled out concretely instead of
/// hidden behind an RNG seed.
///
/// This is the canonical value-level [`Fixture`] format (`holon-pbt-core`), the
/// SINGLE serialization shape for hand-authored regression cases — not a
/// bespoke parallel struct. A case pins `name` / `description` / `transitions`;
/// the `Fixture`-only `environment` and `initial_state` fields are
/// `#[serde(default)]`, so a JSONL line that omits them (as every hand-authored
/// case does — the base is the fixed [`wide_e2e_ref`], not a serialized
/// `ReferenceState`) deserializes cleanly with `initial_state = ()`.
///
/// Upstream convergence: proptest PR #653 (value persistence) is the eventual
/// single-format home for this per the 2026-06-25 decision; Holon pins upstream
/// v1.10.0 (seed-only) until it lands. Do not fold this onto the seed file
/// before then — a seed regression is not hand-authorable (see the module doc
/// above).
type HandAuthoredCase = Fixture<E2ETransition>;

fn sidecar_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(SIDECAR)
}

/// Parse the sidecar, failing loud on the FIRST malformed line (file:line +
/// the raw line + serde's message). No `.ok()` / `_ => skip` — an unparseable
/// hand-authored case is a bug in the case, surfaced immediately.
fn load_cases() -> Vec<HandAuthoredCase> {
    let path = sidecar_path();
    let raw = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("hand-authored regression sidecar {path:?} is unreadable: {e}"));
    raw.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(lineno, line)| {
            serde_json::from_str::<HandAuthoredCase>(line).unwrap_or_else(|e| {
                panic!(
                    "hand-authored regression {path:?}:{lineno} failed to parse: {e}\n  line: \
                     {line}"
                )
            })
        })
        .collect()
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
        // Identical base to `HOLON_PBT_FORCE_FULL=1`: the `full_headless` wiring
        // oracle, so `init_test` boots the full Turso + frontend SUT.
        let initial_state = wide_e2e_ref();
        ComposedSut::<WideE2E>::test_sequential(
            config.clone(),
            initial_state,
            case.transitions,
            None,
        );
        eprintln!("[hand-authored regression] PASSED case {:?}", case.name);
    }
}

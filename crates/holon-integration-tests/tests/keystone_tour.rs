#![cfg(feature = "pbt")]
//! The keystone tour: one fixed replay through the transition alphabet, run by
//! the land gate instead of a random keystone case.
//!
//! A random case draws a few transitions, so whether a landing exercises a
//! given kind depends on the draw. The tour replays [`TOUR`] through the exact
//! keystone harness (`ComposedSut::<WideE2E>` over the `full_headless`
//! wiring), runs the whole catalog after every step, and judges every
//! invariant any step names in `engages` plus `hand_authored::LOCK_FLOOR`. The
//! invariants a step names must give a non-`Skipped` verdict at that step, so a
//! step that stops reaching the state they judge is red rather than a vacuous
//! green. A step whose verdict a `known-red` row of
//! `docs/Testing/KeystoneKnownReds.md` matches is a pass-with-note and the tour
//! goes on; any other red fails it.
//!
//! [`the_tour_covers_the_alphabet`] keeps the tour honest: every kind of
//! `E2ETransition::VARIANT_NAMES` is a tour step or a row of [`NOT_TOURED`].
//!
//! Format of [`TOUR`]: one step per line, `{"engages": [<invariant id>, ...],
//! "transition": <E2ETransition>}`; blank lines and `#` lines are ignored.

use std::collections::BTreeSet;
use std::path::Path;
use std::path::PathBuf;

use holon_integration_tests::pbt::composed::harness::ComposedSut;
use holon_integration_tests::pbt::composed::wide_e2e::WideE2E;
use holon_integration_tests::pbt::composed::wide_e2e::wide_e2e_ref;
use holon_integration_tests::pbt::hand_authored::CheckScope;
use holon_integration_tests::pbt::hand_authored::LOCK_FLOOR;
use holon_integration_tests::pbt::known_reds::KnownReds;
use holon_integration_tests::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::CaptureEnvironment;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

type Machine = <ComposedSut<WideE2E> as StateMachineTest>::Reference;

/// Not `.jsonl`: the `.jsonl` files beside it are `Fixture` corpora, which this
/// format is not.
const TOUR: &str = "hand-authored-regressions/keystone-tour.ndjson";

/// Kinds the tour does not step through, each with the reason.
const NOT_TOURED: &[(&str, &str)] = &[
    ("AdvanceDay", "the full_headless SUT has no SutClockAdvance"),
    (
        "CreateDirectory",
        "a pre-start kind: the tour starts from a started app",
    ),
    (
        "GitInit",
        "a pre-start kind: the tour starts from a started app",
    ),
    (
        "JjGitInit",
        "a pre-start kind: the tour starts from a started app",
    ),
    (
        "CreateStaleLoro",
        "a pre-start kind: the tour starts from a started app",
    ),
    (
        "StartApp",
        "a pre-start kind: the tour starts from a started app",
    ),
    (
        "ShareContainer",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "SharePage",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "SyncNow",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "ReceiverCreateBlock",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "MovePlacedRoot",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "DeletePlacedRoot",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "JoinPlacedRoot",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "DeletePlacementParent",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "DeletePlacementRecord",
        "needs a second instance, which full_headless does not wire",
    ),
    (
        "DeliverBlockContent",
        "only meaningful in a windowed frontend",
    ),
    ("PeerCharEdit", "not migrated to the composed harness"),
    (
        "StructuralKeyBurst",
        "hand-authored cases only; its precondition refuses every other context",
    ),
    (
        "ReleaseIntegrationPeer",
        "needs the integration peer held at session resolve, a boot timing the started tour cannot reach",
    ),
    (
        "SwitchViewMode",
        "full_headless renders no switchable view handle",
    ),
    ("ExpandToggle", "full_headless renders no expand toggle"),
    (
        "MoveUp",
        "needs the navigation focus on a non-page block; full_headless only focuses pages",
    ),
    (
        "MoveDown",
        "needs the navigation focus on a non-page block; full_headless only focuses pages",
    ),
    (
        "JoinBlock",
        "needs the navigation focus on a non-page block; full_headless only focuses pages",
    ),
    ("DragDropBlock", "full_headless renders no draggable block"),
    (
        "FailNextDispatch",
        "red on the cell editor leg: the next create after the failed one diverges",
    ),
    (
        "HoldDispatch",
        "red on the cell editor leg: a held set_field released panics the Loro shadow clock",
    ),
    (
        "ReleaseHeld",
        "red on the cell editor leg: a held set_field released panics the Loro shadow clock",
    ),
    (
        "ExternalWriteSameBlockFocused",
        "red: the editor text and caret diverge from the reference after the external write",
    ),
    (
        "RebootWhileDatabaseHeld",
        "red: block_history is empty after the held-database reboot",
    ),
    (
        "ReleaseDatabaseAndRestart",
        "follows RebootWhileDatabaseHeld, which is not toured",
    ),
];

#[derive(Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct TourStep {
    engages: Vec<String>,
    transition: E2ETransition,
}

fn load_tour(path: &Path) -> Vec<TourStep> {
    let raw = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("keystone tour {path:?} is unreadable: {e}"));
    raw.lines()
        .enumerate()
        .map(|(i, line)| (i + 1, line.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(lineno, line)| {
            let step: TourStep = serde_json::from_str(line)
                .unwrap_or_else(|e| panic!("{path:?}:{lineno} is no tour step: {e}\n  {line}"));
            assert!(
                !step.engages.is_empty(),
                "{path:?}:{lineno}: a step names the invariants it engages, at least one"
            );
            step
        })
        .collect()
}

fn tour_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(TOUR)
}

/// The alphabet kinds neither toured nor excluded, plus every exclusion that
/// names no kind or a toured one.
fn coverage_gaps(
    alphabet: &[&str],
    toured: &BTreeSet<&str>,
    excluded: &[(&str, &str)],
) -> Vec<String> {
    let excluded_kinds: BTreeSet<&str> = excluded.iter().map(|(kind, _)| *kind).collect();
    let untoured = alphabet
        .iter()
        .filter(|kind| !toured.contains(*kind) && !excluded_kinds.contains(*kind))
        .map(|kind| format!("{kind}: in no tour step and not in NOT_TOURED"));
    let stale = excluded.iter().filter_map(|(kind, _)| {
        if !alphabet.contains(kind) {
            Some(format!("{kind}: in NOT_TOURED but not in the alphabet"))
        } else if toured.contains(kind) {
            Some(format!("{kind}: in NOT_TOURED but a tour step draws it"))
        } else {
            None
        }
    });
    untoured.chain(stale).collect()
}

#[test]
fn the_tour_covers_the_alphabet() {
    let tour = load_tour(&tour_path());
    let toured: BTreeSet<&str> = tour.iter().map(|s| s.transition.variant_name()).collect();
    let gaps = coverage_gaps(E2ETransition::VARIANT_NAMES, &toured, NOT_TOURED);
    assert!(
        gaps.is_empty(),
        "{} keystone tour coverage gap(s) in {TOUR}:\n  {}",
        gaps.len(),
        gaps.join("\n  ")
    );
}

#[test]
fn a_kind_dropped_from_the_tour_is_named() {
    let toured = BTreeSet::from(["Indent"]);
    let gaps = coverage_gaps(&["Indent", "Outdent"], &toured, &[]);
    assert_eq!(gaps, ["Outdent: in no tour step and not in NOT_TOURED"]);
}

#[test]
fn an_exclusion_of_a_toured_kind_is_stale() {
    let toured = BTreeSet::from(["Indent"]);
    let gaps = coverage_gaps(&["Indent"], &toured, &[("Indent", "why")]);
    assert_eq!(gaps, ["Indent: in NOT_TOURED but a tour step draws it"]);
}

/// The step's named invariants that gave no non-`Skipped` verdict.
fn unengaged<'a>(engages: &'a [String], engaged: &BTreeSet<&str>) -> Vec<&'a str> {
    engages
        .iter()
        .map(String::as_str)
        .filter(|id| !engaged.contains(id))
        .collect()
}

#[test]
fn a_step_whose_invariants_all_skipped_is_unengaged() {
    let engages = vec!["inv-a".to_string(), "inv-b".to_string()];
    assert_eq!(unengaged(&engages, &BTreeSet::new()), ["inv-a", "inv-b"]);
    assert!(unengaged(&engages, &BTreeSet::from(["inv-a", "inv-b"])).is_empty());
}

/// One scope for every step: a reboot asserts that the judged set survives it.
fn scope_of(tour: &[TourStep]) -> CheckScope {
    CheckScope {
        judged: tour
            .iter()
            .flat_map(|step| step.engages.iter().cloned())
            .chain(LOCK_FLOOR.iter().map(|id| id.to_string()))
            .collect(),
        // `inv-sql-budget` judges a step only after a whole-catalog sweep of the previous state.
        runs_whole_catalog: true,
    }
}

#[test]
#[ignore = "a land gate step, longer than the nextest cap: run with --include-ignored"]
fn keystone_tour() {
    if let Some(report) = CaptureEnvironment::default().mismatch_report(None) {
        panic!("the keystone tour starts from the default environment:\n{report}");
    }
    let tour = load_tour(&tour_path());
    let total = tour.len();
    let scope = scope_of(&tour);
    let mut ref_state = wide_e2e_ref();
    let known_reds = KnownReds::load().expect("known-reds registry");
    let mut known_red_hits = Vec::new();
    let mut sut = ComposedSut::<WideE2E>::init_test(&ref_state);
    sut.check_scoped(&ref_state, &scope);
    for (
        ix,
        TourStep {
            engages,
            transition,
        },
    ) in tour.into_iter().enumerate()
    {
        let kind = transition.variant_name();
        eprintln!("\nApplying transition {}/{total}: {transition:?}", ix + 1);
        assert!(
            Machine::preconditions(&ref_state, &transition),
            "tour step {}/{total} ({kind}) fails its precondition in the state the tour reaches",
            ix + 1
        );
        ref_state = Machine::apply(ref_state, &transition);
        sut = ComposedSut::<WideE2E>::apply(sut, &ref_state, transition);
        // The verdict panic is the check's last statement, so the SUT state
        // stays sound past a known red and the tour goes on from it.
        let checked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            sut.check_scoped(&ref_state, &scope)
        }));
        let engaged = match checked {
            Ok(engaged) => engaged,
            Err(payload) => {
                let message = payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| (*s).to_string()))
                    .unwrap_or_default();
                let Some(key) = known_reds.classify(&message) else {
                    std::panic::resume_unwind(payload)
                };
                known_red_hits.push(format!("step {}/{total} ({kind}): `{key}`", ix + 1));
                eprintln!(
                    "[keystone tour] step {}/{total} {kind} hit known red `{key}`; its engage \
                     check is not judged",
                    ix + 1
                );
                continue;
            }
        };
        let missing = unengaged(&engages, &engaged);
        assert!(
            missing.is_empty(),
            "tour step {}/{total} ({kind}) engaged {} of its {} invariant(s): {missing:?} gave \
             only `Skipped` verdicts, so the step no longer reaches the state they judge",
            ix + 1,
            engages.len() - missing.len(),
            engages.len(),
        );
        eprintln!(
            "[keystone tour] step {}/{total} {kind} engaged {engages:?}",
            ix + 1
        );
    }
    sut.teardown_scoped(ref_state, &scope);
    eprintln!(
        "[keystone tour] {} known red hit(s) in {} (pass-with-note): {known_red_hits:#?}",
        known_red_hits.len(),
        known_reds.registry().display()
    );
}

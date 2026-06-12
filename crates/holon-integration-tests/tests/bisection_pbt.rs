//! ADR 0009 §3a/§4 — cross-`ComponentSet` replay portability + component
//! bisection.
//!
//! Two layers:
//!
//! 1. **§3a portability spike (always runs, no SUT).** The load-bearing
//!    invariant for bisection: replaying a recorded `Vec<E2ETransition>` under
//!    [`ReplayMode::SkipGated`] against a subset's wiring turns gated-out
//!    transitions into deterministic `SkippedByGating` no-ops that are **never**
//!    applied to the reference model — so the engine's applied sequence equals
//!    exactly the node's applicable subsequence, and (since reference `apply` is
//!    pure) the resulting reference state matches a `Strict` replay of that
//!    subsequence. Verified with [`NullStepper`] (no SUT), so it is fast and
//!    deterministic and can guard every CI run.
//!
//! 2. **SUT-backed bisection entry (env-gated).** `bisect_capture_from_env`
//!    drives the real lattice search ([`bisect_capture`]) over a capture file,
//!    building an `E2ESut` per node. It is expensive (a SUT per lattice node), so
//!    it runs only when `HOLON_BISECT_CAPTURE` is set; otherwise it is a no-op.
//!    This is the CLI/env entry of ADR 0009 migration step 4.

#![cfg(feature = "pbt")]

use holon_integration_tests::pbt::bisect_driver::{
    bisect_capture, ceiling_by_name, load_capture, reproduces_under, reproduction_signature,
};
use holon_integration_tests::pbt::fresh_reference_state;
use holon_integration_tests::pbt::stepper::{NullStepper, ReplayMode, StepOutcome, run_sequence};
use holon_integration_tests::pbt::transitions::{E2ETransition, PressKey};
use holon_pbt_core::StorageAdapter;
use holon_pbt_core::component_set::{ComponentSet, Projection};

/// Canonical, order-stable serialization of a transition. Two clones of one
/// source value share the same internal-map hash seed, so their JSON is
/// byte-identical — unlike a whole-`ReferenceState` `Debug`. (`E2ETransition`
/// is not `PartialEq`, hence comparison via serde.)
fn canon(transitions: &[E2ETransition]) -> Vec<String> {
    transitions
        .iter()
        .map(|t| serde_json::to_string(t).expect("transition serializes"))
        .collect()
}

/// Like [`reproduces_under`], but with an explicit reproduction `signature`. A
/// run counts as a reproduction iff it panicked AND the payload *contains* the
/// needle. `Some(needle)` pins an exact failure; `None` falls back to the shared
/// default ([`reproduction_signature`] — the cross-layer `trouble begins at:`
/// divergence marker, or the `HOLON_BISECT_SIGNATURE` override), so this is
/// exactly `reproduces_under`. Either way a panic *without* the signature is a
/// replay-infidelity abort, not a reproduction. The panic hook is muted so
/// expected panics don't spam the log.
fn reproduces_with_signature(
    set: &ComponentSet,
    transitions: &[E2ETransition],
    signature: Option<&str>,
) -> bool {
    let needle = signature
        .map(str::to_string)
        .unwrap_or_else(reproduction_signature);
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let wiring = set.wiring.clone();
    let transitions = transitions.to_vec();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
        let ref0 = fresh_reference_state(wiring);
        let mut stepper = holon_integration_tests::pbt::stepper::BisectionStepper::default();
        run_sequence(&mut stepper, ref0, transitions, None, ReplayMode::SkipGated);
    }));
    std::panic::set_hook(prev);
    match outcome {
        Ok(()) => false,
        Err(payload) => {
            let msg = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or_else(|| {
                    // A non-string payload can't be signature-matched; treating
                    // it as "not reproduced" would exonerate a failing component.
                    panic!(
                        "reproduces_with_signature: panic payload is neither String \
                         nor &str (type_id {:?}) — cannot match signature {needle:?}",
                        payload.type_id()
                    )
                });
            msg.contains(&needle)
        }
    }
}

/// §3a property over one (ceiling, sequence) pair: for the ceiling and every
/// valid child, the engine's `SkipGated` applied sequence equals exactly the
/// node's applicable subsequence (a pure function of `(transition, wiring)`,
/// computed independently of the engine), and every step is accounted for as
/// `Applied` or `SkippedByGating`. Since reference `apply` is pure, an identical
/// applied sequence implies an identical resulting reference state.
fn assert_skip_gated_is_ref_invariant(ceiling: &ComponentSet, transitions: &[E2ETransition]) {
    let nodes = std::iter::once(ceiling.clone()).chain(ceiling.valid_children());
    for node in nodes {
        let applicable: Vec<E2ETransition> = transitions
            .iter()
            .filter(|t| t.required_wiring().satisfied_by(&node.wiring))
            .cloned()
            .collect();

        let mut run = NullStepper::default();
        let outcomes = run_sequence(
            &mut run,
            fresh_reference_state(node.wiring.clone()),
            transitions.to_vec(),
            None,
            ReplayMode::SkipGated,
        );

        let skipped = outcomes
            .iter()
            .filter(|o| **o == StepOutcome::SkippedByGating)
            .count();
        assert_eq!(
            run.applied().len() + skipped,
            transitions.len(),
            "node {node:?}: every step is Applied or SkippedByGating",
        );
        assert_eq!(
            canon(run.applied()),
            canon(&applicable),
            "node {node:?}: the SkipGated applied sequence must equal the node's \
             applicable subsequence (a gated step must never reach `apply`)",
        );
    }
}

/// A committed failing capture replays portably across the whole lattice rooted
/// at `full_headless`: the §4 invariant holds for a real recorded sequence.
#[test]
fn skip_gated_replay_is_portable_for_committed_capture() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/tests/fixtures/general_e2e_pbt/loro-content-drop-set-field.json"
    );
    let transitions = load_capture(path);
    assert!(
        !transitions.is_empty(),
        "committed fixture must carry transitions",
    );
    assert_skip_gated_is_ref_invariant(&ComponentSet::full_headless(), &transitions);
}

/// The skip path is actually exercised: a Loro-only transition (`PressKey`,
/// `RequiredWiring::HasStorage(Loro)`) replayed against a Turso-only node is
/// gated to a `SkippedByGating` no-op that leaves the reference state at its
/// fresh initial value — never applied, never a panic. (The committed-capture
/// test above carries only `Any`-wiring transitions, so it never skips.)
#[test]
fn editor_transition_skips_purely_under_storeless_node() {
    // `PressKey` gates on `AnyStorageOf({Loro, Turso})` (ADR 0009 asymmetry #1),
    // so the node must lack *both* to gate it out. An Org-only node does:
    // `Wiring::org_create_ordering()` = `{Org}`.
    let org_only = ComponentSet::new(
        holon_pbt_core::Wiring::org_create_ordering(),
        [Projection::ViewModel, Projection::EditorState],
    );
    assert!(!org_only.has_storage(StorageAdapter::Loro));
    assert!(!org_only.has_storage(StorageAdapter::Turso));

    let enter = holon_api::KeyChord(std::iter::once(holon_api::Key::Enter).collect());
    let seq = vec![E2ETransition::PressKey(PressKey { chord: enter })];

    let mut run = NullStepper::default();
    let outcomes = run_sequence(
        &mut run,
        fresh_reference_state(org_only.wiring.clone()),
        seq,
        None,
        ReplayMode::SkipGated,
    );
    assert_eq!(
        outcomes,
        vec![StepOutcome::SkippedByGating],
        "an editor transition must be skipped under a node with neither Loro nor Turso",
    );
    assert!(
        run.applied().is_empty(),
        "a SkippedByGating step must never reach `apply` (so it cannot change \
         the reference state)",
    );
}

/// ADR 0009 asymmetry #1 — the editor-transition storage gate. After the change
/// from `HasStorage(Loro)` to `AnyStorageOf({Loro, Turso})`, "edit content" is
/// structurally available under Turso-only wiring (where the on-blur `set_field`
/// path persists it), not only under Loro — so the editor path is bisectable
/// across the storage axis. (Headless Turso-only slices stay unaffected: the
/// transition's `preconditions` gate on `has_editor_buffer()` — the editor
/// capability — so without a wired editor buffer the transition deselects.)
#[test]
fn editor_transitions_gate_on_any_of_loro_or_turso() {
    use holon_integration_tests::pbt::ReferenceState;
    use holon_integration_tests::pbt::transitions::TypeChars;
    use holon_pbt_core::{TransitionFactory, Wiring};

    for req in [
        <PressKey as TransitionFactory<ReferenceState>>::required_wiring(),
        <TypeChars as TransitionFactory<ReferenceState>>::required_wiring(),
    ] {
        assert!(
            req.satisfied_by(&Wiring::sql_only()),
            "available under Turso-only (the on-blur set_field path)",
        );
        assert!(
            req.satisfied_by(&Wiring::loro_backend()),
            "available under Loro-only",
        );
        assert!(
            !req.satisfied_by(&Wiring::org_create_ordering()),
            "gated out when neither Loro nor Turso is wired (Org-only)",
        );
    }
}

/// SUT-backed bisection entry (ADR 0009 §3 migration step 4). No-op unless
/// `HOLON_BISECT_CAPTURE` points at a capture file. Optional `HOLON_BISECT_CEILING`
/// names the ceiling preset the capture was generated under (the capture itself
/// does not record wiring — ADR §4 item 2); defaults to `full_headless`.
///
/// Builds a real `E2ESut` per lattice node, so it is intentionally manual:
///   HOLON_BISECT_CAPTURE=tests/.captures/general_e2e_pbt.captured.json \
///   HOLON_BISECT_CEILING=full_headless \
///   cargo test -p holon-integration-tests --features pbt \
///     --test bisection_pbt bisect_capture_from_env -- --nocapture
#[test]
#[ignore = "manual tool: set HOLON_BISECT_CAPTURE or HOLON_BISECT_SLICE, run with --ignored"]
fn bisect_capture_from_env() {
    // Two ways to name the capture: an explicit path (`HOLON_BISECT_CAPTURE`), or
    // a slice name (`HOLON_BISECT_SLICE`) resolved to its auto-written capture
    // under `tests/.captures/<slice>.captured.json`. The slice form is the CI
    // triage entry: on a red wide PBT, re-run with `HOLON_BISECT_SLICE=<slice>`
    // to localize the just-written capture.
    let capture_path = match (
        std::env::var("HOLON_BISECT_CAPTURE"),
        std::env::var("HOLON_BISECT_SLICE"),
    ) {
        (Ok(path), _) => path,
        (Err(_), Ok(slice)) => format!(
            "{}/tests/.captures/{slice}.captured.json",
            env!("CARGO_MANIFEST_DIR")
        ),
        (Err(_), Err(_)) => {
            eprintln!(
                "[bisect] neither HOLON_BISECT_CAPTURE nor HOLON_BISECT_SLICE set — \
                 skipping SUT-backed bisection (set one to localize a capture)",
            );
            return;
        }
    };
    let fixture = holon_integration_tests::pbt::bisect_driver::load_capture_fixture(&capture_path);
    let transitions = fixture.transitions;
    // Ceiling precedence: explicit env > the capture's recorded wiring >
    // full_headless. A capture replayed under a different ceiling than it was
    // generated under is the documented replay-infidelity trap.
    let (ceiling, ceiling_name) = match std::env::var("HOLON_BISECT_CEILING") {
        Ok(name) => (ceiling_by_name(&name), name),
        Err(_) => match &fixture.environment.wiring {
            Some(w) if *w == holon_pbt_core::Wiring::sql_only() => (
                ceiling_by_name("sql_only"),
                "sql_only (from capture)".into(),
            ),
            Some(w) if *w == holon_pbt_core::Wiring::full() => (
                ceiling_by_name("full_headless"),
                "full_headless (from capture)".into(),
            ),
            Some(w) => panic!(
                "[bisect] capture wiring {w:?} matches no ceiling preset — \
                 set HOLON_BISECT_CEILING explicitly"
            ),
            None => (ceiling_by_name("full_headless"), "full_headless".into()),
        },
    };

    // Probe mode: just report whether the ceiling reproduces (one SUT build),
    // without walking the lattice. Cheap triage before committing to a full
    // (many-SUT) bisect.
    if std::env::var("HOLON_BISECT_PROBE").is_ok() {
        let reproduced = reproduces_under(&ceiling, &transitions);
        eprintln!(
            "[bisect] probe {capture_path} ({} transitions) under {ceiling_name}: \
             reproduces = {reproduced}",
            transitions.len(),
        );
        return;
    }

    eprintln!(
        "[bisect] localizing {capture_path} ({} transitions) under ceiling {ceiling_name}",
        transitions.len(),
    );
    let localization = bisect_capture(ceiling, &transitions);
    eprintln!("[bisect] localization: {localization:?}");
}

/// Verbose replay: run a capture under `ceiling` WITHOUT muting the panic hook,
/// catch the panic, and print its payload — so a minimized capture's failure
/// *signature* can be compared against the original (ddmin's oracle only checks
/// "still fails", not "fails the same way", so it can over-minimize into a
/// different failure mode).
///
/// ```text
///   HOLON_BISECT_CAPTURE=tests/.captures/gpui_ui_pbt.captured.json.min.json \
///   HOLON_BISECT_CEILING=loro_vm_fast \
///     cargo test -p holon-integration-tests --features pbt \
///     --test bisection_pbt replay_capture_verbose_from_env -- --nocapture
/// ```
#[test]
#[ignore = "manual tool: set HOLON_BISECT_CAPTURE or HOLON_BISECT_SLICE, run with --ignored"]
fn replay_capture_verbose_from_env() {
    let capture_path = match (
        std::env::var("HOLON_BISECT_CAPTURE"),
        std::env::var("HOLON_BISECT_SLICE"),
    ) {
        (Ok(path), _) => path,
        (Err(_), Ok(slice)) => format!(
            "{}/tests/.captures/{slice}.captured.json",
            env!("CARGO_MANIFEST_DIR")
        ),
        (Err(_), Err(_)) => {
            eprintln!(
                "[replay] neither HOLON_BISECT_CAPTURE nor HOLON_BISECT_SLICE set — skipping"
            );
            return;
        }
    };
    let ceiling_name =
        std::env::var("HOLON_BISECT_CEILING").unwrap_or_else(|_| "loro_vm_fast".to_string());
    let ceiling = ceiling_by_name(&ceiling_name);
    let transitions = load_capture(&capture_path);
    eprintln!(
        "[replay] replaying {capture_path} ({} transitions) under {ceiling_name}",
        transitions.len(),
    );
    // Optional: require the panic payload to contain this needle to count as a
    // faithful reproduction (ddmin's oracle is failure-agnostic; this makes the
    // signature explicit). Optional repeat to measure flakiness (Heisenbugs).
    let signature = std::env::var("HOLON_BISECT_SIGNATURE").ok();
    let repeat: u32 = std::env::var("HOLON_BISECT_REPEAT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);

    let mut signature_hits = 0u32;
    let mut other_panics = 0u32;
    let mut clean_runs = 0u32;
    for i in 0..repeat {
        let transitions = transitions.clone();
        let wiring = ceiling.wiring.clone();
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let ref0 = fresh_reference_state(wiring);
            let mut stepper = holon_integration_tests::pbt::stepper::BisectionStepper::default();
            run_sequence(&mut stepper, ref0, transitions, None, ReplayMode::SkipGated);
        }));
        match outcome {
            Ok(()) => {
                clean_runs += 1;
                eprintln!("[replay] run {i}: ran to completion — DID NOT reproduce");
            }
            Err(payload) => {
                let msg = payload
                    .downcast_ref::<String>()
                    .map(String::as_str)
                    .or_else(|| payload.downcast_ref::<&str>().copied())
                    .unwrap_or("<non-string panic payload>");
                let first = msg.lines().next().unwrap_or(msg);
                match &signature {
                    Some(needle) if msg.contains(needle.as_str()) => {
                        signature_hits += 1;
                        eprintln!("[replay] run {i}: REPRODUCED (signature match) — {first}");
                    }
                    Some(needle) => {
                        other_panics += 1;
                        eprintln!(
                            "[replay] run {i}: panicked but NOT signature {needle:?} — {first}"
                        );
                    }
                    None => {
                        other_panics += 1;
                        eprintln!("[replay] run {i}: REPRODUCED — {first}");
                    }
                }
            }
        }
    }
    eprintln!(
        "[replay] tally over {repeat} run(s) under {ceiling_name}: \
         signature_hits={signature_hits} other_panics={other_panics} clean={clean_runs} \
         signature={signature:?}",
    );
}

/// Sequence minimizer (greedy ddmin) for a recorded failing capture: drop one
/// transition at a time, keep every drop that still reproduces the failure under
/// `ceiling`, iterate to a fixpoint. The result is a locally-minimal failing
/// subsequence — fewer transitions to read than the raw capture. Writes the
/// minimized fixture next to the input as `<capture>.min.json`.
///
/// Uses the same cheap, in-process oracle as the bisector, so minimization is
/// just a delta-debug over the sequence axis (orthogonal to the component-lattice
/// axis the bisector minimizes).
///
/// **Signature guard.** Set `HOLON_BISECT_SIGNATURE=<substr>` to require the
/// panic payload to *contain* that substring — without it the oracle is
/// failure-agnostic and ddmin happily collapses into an unrelated failure mode
/// (e.g. a reference-model `unwrap` on a block whose creating transition got
/// dropped). For the gpui SplitBlock capture, pass
/// `HOLON_BISECT_SIGNATURE=inv-blocks-match-ref/loro`.
///
/// ```text
///   HOLON_BISECT_SLICE=gpui_ui_pbt HOLON_BISECT_CEILING=loro_vm_fast \
///     cargo test -p holon-integration-tests --features pbt \
///     --test bisection_pbt minimize_capture_from_env -- --nocapture
/// ```
#[test]
#[ignore = "manual tool: set HOLON_BISECT_CAPTURE or HOLON_BISECT_SLICE, run with --ignored"]
fn minimize_capture_from_env() {
    let capture_path = match (
        std::env::var("HOLON_BISECT_CAPTURE"),
        std::env::var("HOLON_BISECT_SLICE"),
    ) {
        (Ok(path), _) => path,
        (Err(_), Ok(slice)) => format!(
            "{}/tests/.captures/{slice}.captured.json",
            env!("CARGO_MANIFEST_DIR")
        ),
        (Err(_), Err(_)) => {
            eprintln!(
                "[minimize] neither HOLON_BISECT_CAPTURE nor HOLON_BISECT_SLICE set — skipping",
            );
            return;
        }
    };
    let ceiling_name =
        std::env::var("HOLON_BISECT_CEILING").unwrap_or_else(|_| "loro_vm_fast".to_string());
    let ceiling = ceiling_by_name(&ceiling_name);
    let signature = std::env::var("HOLON_BISECT_SIGNATURE").ok();
    let capture_fixture =
        holon_integration_tests::pbt::bisect_driver::load_capture_fixture(&capture_path);
    let capture_environment = capture_fixture.environment.clone();
    let mut seq = capture_fixture.transitions;

    let repro = |s: &[E2ETransition]| reproduces_with_signature(&ceiling, s, signature.as_deref());

    if !repro(&seq) {
        eprintln!(
            "[minimize] capture does NOT reproduce under {ceiling_name} ({} transitions) with \
             signature={signature:?} — cannot minimize headlessly. This failure is runner-coupled \
             (only the real-window gpui runner reproduces it); the headless component oracle never \
             hits its signature.",
            seq.len(),
        );
        return;
    }
    eprintln!(
        "[minimize] start: {} transitions reproduce under {ceiling_name} (signature={signature:?})",
        seq.len(),
    );

    // Greedy single-element delta-debug to a fixpoint.
    loop {
        let mut shrunk = false;
        let mut i = 0;
        while i < seq.len() {
            let mut candidate = seq.clone();
            let dropped = candidate.remove(i);
            if repro(&candidate) {
                eprintln!(
                    "[minimize] dropped #{i} {} → {} transitions still reproduce",
                    serde_json::to_string(&dropped).unwrap_or_default(),
                    candidate.len(),
                );
                seq = candidate;
                shrunk = true;
            } else {
                i += 1;
            }
        }
        if !shrunk {
            break;
        }
    }

    eprintln!(
        "[minimize] minimal: {} transitions still reproduce under {ceiling_name}",
        seq.len(),
    );
    for (i, t) in seq.iter().enumerate() {
        eprintln!(
            "[minimize]   [{i}] {}",
            serde_json::to_string(t).unwrap_or_default()
        );
    }

    let out_path = format!("{capture_path}.min.json");
    let fixture: holon_pbt_core::fixture::Fixture<E2ETransition, ()> =
        holon_pbt_core::fixture::Fixture {
            name: format!("minimized from {capture_path} under {ceiling_name}"),
            description: "ddmin-minimized failing subsequence".to_string(),
            environment: capture_environment,
            initial_state: (),
            transitions: seq,
        };
    fixture
        .save(&out_path)
        .unwrap_or_else(|e| panic!("[minimize] save {out_path:?}: {e}"));
    eprintln!("[minimize] wrote minimized capture to {out_path}");
}

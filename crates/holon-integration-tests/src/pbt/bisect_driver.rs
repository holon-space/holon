//! SUT-backed component bisection (ADR 0009 §3/§4): replay a recorded failing
//! sequence across a `ComponentSet` lattice and localize the smallest set that
//! still reproduces the failure.
//!
//! The pure lattice search lives in [`holon_pbt_core::bisect`]; it takes a node
//! oracle `FnMut(&ComponentSet) -> bool`. This module is the thin adapter that
//! makes that oracle *real*: per node it builds an `E2ESut` wired for the node
//! (via [`BisectionStepper`]), replays the captured `Vec<E2ETransition>` under
//! [`ReplayMode::SkipGated`] — so transitions the node gates out become
//! deterministic skips, never panics — and reports whether the run still
//! failed.
//!
//! **Honest framing (ADR 0009 §3).** "Reproduces" means the reference model and
//! the node's enabled projections *disagree* (an invariant failed / the SUT
//! aborted). It does **not** assert which side is correct; in this codebase the
//! divergence has often been a reference-model fidelity gap, not a prod bug.
//! The bisector localizes *where* the disagreement enters; *who is wrong* is a
//! human step.

use std::panic::AssertUnwindSafe;
use std::panic::catch_unwind;
use std::path::Path;

use holon_pbt_core::bisect::Localization;
use holon_pbt_core::bisect::bisect;
use holon_pbt_core::component_set::ComponentSet;
use holon_pbt_core::fixture::Fixture;

use crate::pbt::composed::wide_e2e::wide_e2e_ref_for;
use crate::pbt::stepper::BisectionStepper;
use crate::pbt::stepper::ReplayMode;
use crate::pbt::stepper::run_sequence;
use crate::pbt::transitions::E2ETransition;

/// Load a captured failing sequence — the `tests/.captures/*.captured.json`
/// artifact written by the slice wrapper's `Drop` on a panicking case
/// (a serde-tagged `Vec<E2ETransition>`). Fails loud on IO/parse error: a
/// malformed capture must not silently localize to nothing.
///
/// Discloses the recorded [`CaptureEnvironment`] and warns when the current
/// process env differs — a capture replayed under different editor-shape
/// flags or wiring silently changes meaning (ADR 0009 §4 follow-up).
pub fn load_capture(path: impl AsRef<Path>) -> Vec<E2ETransition> {
    load_capture_fixture(path).transitions
}

/// Like [`load_capture`] but returns the whole fixture (incl. `environment`)
/// so replayers can derive the ceiling from the recorded wiring.
pub fn load_capture_fixture(path: impl AsRef<Path>) -> Fixture<E2ETransition, ()> {
    let path = path.as_ref();
    let fixture: Fixture<E2ETransition, ()> =
        Fixture::load(path).unwrap_or_else(|e| panic!("[bisect] load capture {path:?}: {e}"));
    match &fixture.environment.wiring {
        Some(w) => eprintln!("[bisect] capture environment: wiring={w:?}"),
        None => eprintln!("[bisect] capture has no environment header (pre-header capture)"),
    }
    if let Some(report) = fixture.environment.mismatch_report(None) {
        eprintln!(
            "[bisect] WARNING: replay env differs from capture env — replay may be \
             unfaithful:\n{report}"
        );
    }
    fixture
}

/// Replay `transitions` against a SUT wired for `set` under
/// [`ReplayMode::SkipGated`] and report whether the **reference and the node's
/// enabled projections diverge**. The capture carries no wiring (ADR 0009 §4
/// item 2); `set` supplies it.
///
/// "Reproduces" is *not* "the run panicked" — it is "an invariant diverged".
/// Invariant failures funnel through `format_layer_report` and always carry the
/// `trouble begins at:` marker; a panic *without* it is a **replay-infidelity
/// abort**, not a divergence: a transition whose SUT path is Turso-coupled
/// (e.g. the `probe_block_sql_state` diagnostic, or a settle timeout) crashing
/// on a no-Turso node it was never generated against. Those are logged and
/// counted as non-reproduction, so the downward walk never *descends into* a
/// node where the capture cannot be faithfully replayed and spuriously
/// localizes there (this is exactly the loro-node false positive that made a
/// Turso-generated capture "reproduce" under `loro_vm_fast`). Trade-off: a
/// genuine non-invariant SUT crash also lacks the marker and is not counted —
/// bisection localizes *divergence*, which is its stated purpose (ADR §3).
///
/// `HOLON_BISECT_VERBOSE=1` keeps the default panic hook so the panic message +
/// backtrace print (to tell a real divergence from an infidelity abort).
pub fn reproduces_under(set: &ComponentSet, transitions: &[E2ETransition]) -> bool {
    let verbose = std::env::var("HOLON_BISECT_VERBOSE").is_ok();
    let prev_hook = std::panic::take_hook();
    if !verbose {
        std::panic::set_hook(Box::new(|_| {}));
    }
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        // Seed the reference to match the composed `ComposedSut<WideE2E>` the
        // `BisectionStepper` builds (`wide_e2e_ref_for` == the keystone's seeded init
        // for this wiring), so reference and SUT start in lockstep — an empty
        // `fresh_reference_state` would make every capture spuriously diverge on the
        // seed.
        let ref0 = wide_e2e_ref_for(&set.wiring);
        let mut stepper = BisectionStepper::default();
        run_sequence(
            &mut stepper,
            ref0,
            transitions.to_vec(),
            None,
            ReplayMode::SkipGated,
        );
    }));
    std::panic::set_hook(prev_hook);

    let Err(payload) = outcome else {
        return false; // ran clean → no divergence
    };
    let msg = panic_message(&payload);
    // The reproduction *signature*. Default: the cross-layer report marker
    // (`format_layer_report`'s `trouble begins at:`), present iff an invariant
    // actually diverged. Override with `HOLON_BISECT_SIGNATURE` to pin an exact
    // failure message (so ddmin/bisection localize *that* bug, not a different
    // failure mode the over-shrunk capture happened to hit).
    let signature = reproduction_signature();
    let is_divergence = msg.contains(&signature);
    if !is_divergence {
        let first_line = msg.lines().next().unwrap_or("<no message>");
        eprintln!(
            "[bisect] {:?}+{:?}: replay aborted, NOT a reproduction (inconclusive node) — \
             {first_line}",
            set.wiring.storage_adapters, set.projections,
        );
    }
    is_divergence
}

/// The default reproduction signature — the cross-layer divergence marker — or
/// the `HOLON_BISECT_SIGNATURE` override. A caught panic counts as a
/// reproduction iff its message contains this; anything else is a
/// replay-infidelity abort.
pub fn reproduction_signature() -> String {
    std::env::var("HOLON_BISECT_SIGNATURE")
        .unwrap_or_else(|_| "reconciled composed sequence diverged from the oracle".to_string())
}

/// Extract the panic message from a caught payload (`panic!`/`format!` produce
/// a `String`; `expect`/`&str` literals a `&str`).
fn panic_message(payload: &Box<dyn std::any::Any + Send>) -> String {
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        (*s).to_string()
    } else {
        "<panic with non-string payload>".to_string()
    }
}

/// Bisect a capture across the lattice rooted at `ceiling`, localizing the
/// smallest reproducing `ComponentSet` (ADR 0009 §3, bidirectional). The floor
/// for the upward (absent-component) search is the ceiling's minimal valid
/// descendant.
pub fn bisect_capture(ceiling: ComponentSet, transitions: &[E2ETransition]) -> Localization {
    let floor = minimal_descendant(&ceiling);
    bisect(ceiling, floor, |set| reproduces_under(set, transitions))
}

/// A deterministic minimal valid descendant of `set`: greedily drop the first
/// valid child until none remain. Used as the floor for the upward search.
fn minimal_descendant(set: &ComponentSet) -> ComponentSet {
    let mut current = set.clone();
    while let Some(child) = current.valid_children().into_iter().next() {
        current = child;
    }
    current
}

/// Resolve a blessed-preset name (the ceiling the capture was generated under,
/// which the capture itself does not record) to a [`ComponentSet`]. Used by the
/// env-driven entry point.
pub fn ceiling_by_name(name: &str) -> ComponentSet {
    match name {
        "full_gpui" => ComponentSet::full_gpui(),
        "full_headless" => ComponentSet::full_headless(),
        "loro_vm_fast" => ComponentSet::loro_vm_fast(),
        "sql_only" => ComponentSet::sql_only(),
        other => panic!(
            "[bisect] unknown ceiling preset {other:?} (expected one of: full_gpui, \
             full_headless, loro_vm_fast, sql_only)"
        ),
    }
}

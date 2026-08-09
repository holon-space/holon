//! The `Stepper` seam: one sequential-loop engine shared by the headless
//! proptest runner and the phased GPUI runner (ADR 0009 §5).
//!
//! `proptest-state-machine`'s `test_sequential` is a plain synchronous
//! `for`-loop (init → for each transition: `ref.apply`, `sut.apply`,
//! `check_invariants` → teardown) with **no thread affinity** — the crate
//! spawns nothing. The main-thread constraint is entirely GPUI's window
//! event loop, not proptest's. So the loop *body* can be shared; only the
//! per-step interaction with the SUT differs:
//!
//! - headless: call the `E2ESut` directly (it `block_on`s its own runtime),
//! - GPUI: post the step to the main-thread window and block on an ack.
//!
//! This module factors that difference behind [`Stepper`] and reuses
//! [`run_sequence`] for both. The reference model (backend-blind, pure) is
//! advanced by the engine, never by a `Stepper`.
//!
//! Prototype status: `HeadlessStepper` is wired against the real `E2ESut`.
//! `GpuiStepper` is structured against a [`MainThreadBridge`] trait so it
//! compiles without pulling the GPUI crate into this one; the bridge impl
//! lives in `frontends/gpui` and is sketched in the doc comment there.

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

use crate::pbt::composed::harness::ComposedSut;
use crate::pbt::composed::wide_e2e::WideE2E;
use crate::pbt::reference_state::ReferenceState;
use crate::pbt::state_machine::ReferenceMachine;
use crate::pbt::transitions::E2ETransition;

/// Whether a replayed transition that the current wiring gates out is a hard
/// error (the existing `replay_steps` behaviour) or a deterministic skip
/// (required for cross-`ComponentSet` bisection, ADR 0009 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayMode {
    /// Generation and same-set fixture replay: every transition in the
    /// sequence must be applicable. An inapplicable one is a bug in the
    /// fixture/strategy and panics ("fixture encodes a stale assumption").
    Strict,
    /// Cross-set replay (bisection): a transition gated out by this node's
    /// wiring becomes a [`StepOutcome::SkippedByGating`] no-op. The
    /// reference state is **not** advanced for a skipped step.
    SkipGated,
}

/// Per-step result, recorded so a gating-skip on a subset is distinguishable
/// from a genuine divergence (ADR 0009 §4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepOutcome {
    Applied,
    SkippedByGating,
}

/// The variation point between runners. The reference type is fixed
/// (`ReferenceMachine`); a `Stepper` owns the concrete SUT (or a handle to
/// one living on another thread) and runs each phase, bridging threads if it
/// must. Every method is synchronous — the GPUI impl turns its async/main-
/// thread work into a blocking round-trip, which the sequential engine
/// tolerates by construction.
pub trait Stepper {
    /// Build/initialise the SUT for a fresh case (≡
    /// `StateMachineTest::init_test`). For GPUI this may bind to the
    /// already-running main-thread window.
    fn init(&mut self, ref_state: &ReferenceState);

    /// Apply one transition to the SUT. `ref_state` has already been advanced
    /// by the engine. Returns [`StepOutcome::Applied`] (gating is decided by
    /// the engine before this is called, so a `Stepper` only ever applies).
    fn apply(&mut self, ref_state: &ReferenceState, transition: &E2ETransition);

    /// Check invariants against the current SUT + ref state
    /// (≡ `StateMachineTest::check_invariants`).
    fn check_invariants(&mut self, ref_state: &ReferenceState);

    /// Tear down the SUT (≡ `StateMachineTest::teardown`). Consumes self.
    fn teardown(self: Box<Self>) {}

    /// End-of-case hook, called by [`run_sequence`] after the last
    /// transition's `check_invariants` with the FINAL reference state.
    /// `SmtStepper` routes this to `StateMachineTest::teardown`, where the
    /// native E2E impl runs the `NotNavOnly`-gated invariants once so a
    /// nav-tail sequence doesn't end unchecked. Default no-op (bisection
    /// steppers replay pinned signatures and must not add checks).
    fn finish(&mut self, ref_state: &ReferenceState) {
        let _ = ref_state;
    }
}

/// The shared engine — `proptest-state-machine`'s `test_sequential`, factored
/// over [`Stepper`] and extended with [`ReplayMode`] gating (ADR 0009 §4/§5).
///
/// Returns the per-step outcomes so callers (bisection) can tell skips from
/// applies. Panics propagate exactly as `test_sequential`'s do — an invariant
/// failure or SUT panic unwinds, and the slice wrapper's `Drop` still writes
/// the JSON capture.
pub fn run_sequence<S: Stepper>(
    stepper: &mut S,
    mut ref_state: ReferenceState,
    transitions: Vec<E2ETransition>,
    mut seen_counter: Option<Arc<AtomicUsize>>,
    mode: ReplayMode,
) -> Vec<StepOutcome> {
    stepper.init(&ref_state);
    stepper.check_invariants(&ref_state);

    let mut outcomes = Vec::with_capacity(transitions.len());
    let mut timing = StepTimingAgg::default();
    for transition in transitions {
        // Shrink bookkeeping: identical to the library loop — increment
        // before applying so the strategy's first shrink step can drop
        // never-applied transitions.
        if let Some(counter) = seen_counter.as_mut() {
            counter.fetch_add(1, Ordering::SeqCst);
        }

        if mode == ReplayMode::SkipGated && !transition_applicable(&ref_state, &transition) {
            // ADR 0009 §4: a gated step is a deterministic no-op and MUST NOT
            // advance the reference state. The §3a spike asserts exactly this.
            outcomes.push(StepOutcome::SkippedByGating);
            continue;
        }

        // Reference application is backend-blind and pure — it belongs to the
        // engine, never to a Stepper (ADR 0007 §"Reference fragments").
        ref_state = <ReferenceMachine as ReferenceStateMachine>::apply(ref_state, &transition);
        let t_apply = std::time::Instant::now();
        stepper.apply(&ref_state, &transition);
        let apply_ms = t_apply.elapsed().as_millis();
        let t_check = std::time::Instant::now();
        stepper.check_invariants(&ref_state);
        let check_ms = t_check.elapsed().as_millis();
        timing.record(transition.variant_name(), apply_ms, check_ms);
        if step_timing_enabled() {
            eprintln!(
                "[step_timing] step={} {} apply_ms={apply_ms} check_ms={check_ms}",
                outcomes.len() + 1,
                transition.variant_name(),
            );
        }
        outcomes.push(StepOutcome::Applied);
    }
    timing.finish("run_sequence");
    stepper.finish(&ref_state);
    outcomes
}

/// Is this transition offered under `ref_state`'s wiring AND cap set? Mirrors
/// the alphabet gate in `aggregate_transitions` (`transition_dispatch.rs`), but
/// value-level so it can be evaluated during replay. Relies on the value-level
/// `E2ETransition::required_wiring` + `required_caps` added in
/// `transition_dispatch.rs`. Also the shrink-time gate: `WideE2EMachine`'s
/// `preconditions` calls it so a shrunk initial state cannot keep a transition
/// its CapMap has no provider for.
pub(crate) fn transition_applicable(
    ref_state: &ReferenceState,
    transition: &E2ETransition,
) -> bool {
    transition
        .required_wiring()
        .satisfied_by(&ref_state.harness.wiring)
        && ref_state.caps_available(&transition.required_caps())
}

/// `HOLON_PBT_STEP_TIMING=1` prints per-step apply/check wall times — the
/// shared profiling switch for comparing per-transition cost across runners
/// (headless slices via [`run_sequence`], GPUI fixture replay via
/// `fixtures::replay_steps`).
pub fn step_timing_enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("HOLON_PBT_STEP_TIMING")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    })
}

/// `HOLON_PBT_STEP_BUDGET_MS=<n>` — enforce a mean per-transition wall budget
/// (apply + check, StartApp excluded) at the end of every replayed sequence.
/// Unset → no enforcement. CI sets this per runner to pin the gpui/headless
/// cost ratio: with headless `general_e2e_pbt` at ~230 ms/transition, the gpui
/// jobs set ~1.5× that (≈350) to fail when the windowed runner regresses
/// relative to headless.
fn step_budget_ms() -> Option<u128> {
    static BUDGET: std::sync::OnceLock<Option<u128>> = std::sync::OnceLock::new();
    *BUDGET.get_or_init(|| {
        std::env::var("HOLON_PBT_STEP_BUDGET_MS").ok().map(|v| {
            v.parse()
                .expect("HOLON_PBT_STEP_BUDGET_MS must be an integer (milliseconds)")
        })
    })
}

/// Per-sequence aggregate of the `[step_timing]` samples. Records every
/// applied transition except StartApp (its one-time startup cost would drown
/// the per-transition mean) and, in [`finish`](Self::finish), prints the mean
/// and enforces [`step_budget_ms`] when set.
#[derive(Default)]
pub struct StepTimingAgg {
    steps: usize,
    total_ms: u128,
}

impl StepTimingAgg {
    pub fn record(&mut self, variant_name: &str, apply_ms: u128, check_ms: u128) {
        if variant_name == "StartApp" {
            return;
        }
        self.steps += 1;
        self.total_ms += apply_ms + check_ms;
    }

    /// Print the aggregate (when step timing is on or a budget is set) and
    /// panic when the mean per-transition cost exceeds
    /// `HOLON_PBT_STEP_BUDGET_MS`.
    pub fn finish(&self, label: &str) {
        if self.steps == 0 {
            return;
        }
        let mean_ms = self.total_ms / self.steps as u128;
        if step_timing_enabled() || step_budget_ms().is_some() {
            eprintln!(
                "[step_timing] {label}: {} transition(s), total={}ms, mean={mean_ms}ms/transition \
                 (StartApp excluded)",
                self.steps, self.total_ms
            );
        }
        if let Some(budget) = step_budget_ms() {
            assert!(
                mean_ms <= budget,
                "[{label}] per-transition budget exceeded: mean {mean_ms}ms > \
                 HOLON_PBT_STEP_BUDGET_MS={budget}ms over {} transition(s) (apply+check, StartApp \
                 excluded)",
                self.steps
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Null stepper — no SUT. Used by the §3a portability spike to exercise the
// engine's gating/skip semantics in isolation, fast and deterministically. It
// records the transitions the engine actually *applied* (a gated step is never
// passed to `apply`, by construction of `run_sequence`). Because the reference
// `apply` is pure and deterministic, an identical applied sequence implies an
// identical resulting reference state — so "the applied sequence equals the
// node's applicable subsequence" is the §4 property without a brittle whole-
// state comparison (`ReferenceState` is not `PartialEq`, and its `Debug`
// embeds the interpreter's hash-ordered builder list, which is unstable across
// instances).
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct NullStepper {
    applied: Vec<E2ETransition>,
}

impl NullStepper {
    /// The transitions the engine passed to `apply`, in order — i.e. every step
    /// that was *not* a `SkippedByGating` no-op.
    pub fn applied(&self) -> &[E2ETransition] {
        &self.applied
    }
}

impl Stepper for NullStepper {
    fn init(&mut self, _: &ReferenceState) {}

    fn apply(&mut self, _: &ReferenceState, transition: &E2ETransition) {
        self.applied.push(transition.clone());
    }

    fn check_invariants(&mut self, _: &ReferenceState) {}
}

// ---------------------------------------------------------------------------
// Bisection stepper — the headless lattice-node oracle (ADR 0009 §3/§4).
//
// `SmtStepper<E2ESut>` cannot serve a lattice node: `E2ESut::init_test` is
// hard-wired to `StorageSelector::Turso` (it calls `E2ESut::new`), so it
// ignores the node's wiring. `BisectionStepper` wraps the SAME composed
// `ComposedSut<WideE2E>` the keystone `general_e2e_composed_pbt` drives: it
// builds a per-node composed `CapMap` from the node's wiring and runs the full
// composed catalog via `run_selected`. The same captured `Vec<E2ETransition>`
// can then replay against every node via `run_sequence(.., SkipGated)`:
// transitions the node gates out become `SkippedByGating` no-ops; everything
// else applies through the composed dispatch and is checked.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct BisectionStepper {
    sut: Option<ComposedSut<WideE2E>>,
}

impl Stepper for BisectionStepper {
    fn init(&mut self, ref_state: &ReferenceState) {
        // Build the composed `CapMap` for *this node's* wiring — the whole point of
        // bisection. `ComposedSut::init_test` runs `compose_sut` for
        // `ref_state.wiring`, and dropping any previous SUT here releases its
        // DB/session Arcs.
        self.sut = Some(ComposedSut::<WideE2E>::init_test(ref_state));
    }

    fn apply(&mut self, ref_state: &ReferenceState, transition: &E2ETransition) {
        let sut = self.sut.take().expect("init() runs before apply()");
        self.sut = Some(ComposedSut::<WideE2E>::apply(
            sut,
            ref_state,
            transition.clone(),
        ));
    }

    fn check_invariants(&mut self, ref_state: &ReferenceState) {
        // Divergence surfaces as the composed panic
        // ("reconciled composed sequence diverged from the oracle"), which is
        // `bisect_driver::reproduction_signature()`'s default marker.
        let sut = self.sut.as_ref().expect("init() runs before check()");
        ComposedSut::<WideE2E>::check_invariants(sut, ref_state);
    }
}

// ---------------------------------------------------------------------------
// Integration points:
//
// 1. Proptest-macro path (slice.rs). DONE — both `__declare_pbt_slice_wrapper!`
//    and `__declare_pbt_full_slice!` override
//    `StateMachineTest::test_sequential` to call
//    `run_via_state_machine_test::<Self>(..)`, so every blessed slice's
//    per-case loop now runs through `run_sequence`. Generation, shrinking, and
//    `.proptest-regressions` replay are unchanged (they live in
//    `sequential_strategy`, above the loop).
//
// 2. Live windowed generators (increment 4c): the composed windowed loop
//    (`frontends/gpui/tests/gpui_composed_windowed_loop.rs`) and the TUI
//    composed runner (`frontends/tui/tests/common/pbt_main.rs`) drive generated
//    sequences directly through `ComposedSut::<WideE2E>::apply` /
//    `check_invariants` over a windowed boot — the phased GPUI generator they
//    replaced was hand-rolled and is deletion-scheduled with phased.rs (Phase
//    2).
//
// 3. GPUI replay + bisection (ADR 0009 §3/§5). `GpuiReplayStepper` re-checks a
//    captured `Vec<E2ETransition>` on a launched window: let mut s =
//    GpuiReplayStepper::new(&runtime, &mut sut, driver); run_sequence(&mut s,
//    ref0, captured, None, ReplayMode::SkipGated); The same capture replays
//    across a ComponentSet lattice (headless nodes via `SmtStepper`, the UI
//    node via `GpuiReplayStepper`) to localise the smallest reproducing set.
// ---------------------------------------------------------------------------

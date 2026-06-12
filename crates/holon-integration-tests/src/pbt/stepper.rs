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
use std::sync::atomic::{AtomicUsize, Ordering};

use proptest_state_machine::ReferenceStateMachine;

use crate::pbt::reference_state::ReferenceState;
use crate::pbt::state_machine::ReferenceMachine;
use crate::pbt::sut::E2ESut;
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
    /// Build/initialise the SUT for a fresh case (≡ `StateMachineTest::init_test`).
    /// For GPUI this may bind to the already-running main-thread window.
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

/// Is this transition offered under `ref_state`'s wiring AND cap set? Mirrors the
/// alphabet gate in `aggregate_transitions` (`transition_dispatch.rs`), but value-level
/// so it can be evaluated during replay. Relies on the value-level
/// `E2ETransition::required_wiring` + `required_caps` added in `transition_dispatch.rs`.
fn transition_applicable(ref_state: &ReferenceState, transition: &E2ETransition) -> bool {
    transition.required_wiring().satisfied_by(&ref_state.wiring)
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
                "[step_timing] {label}: {} transition(s), total={}ms, \
                 mean={mean_ms}ms/transition (StartApp excluded)",
                self.steps, self.total_ms
            );
        }
        if let Some(budget) = step_budget_ms() {
            assert!(
                mean_ms <= budget,
                "[{label}] per-transition budget exceeded: mean {mean_ms}ms > \
                 HOLON_PBT_STEP_BUDGET_MS={budget}ms over {} transition(s) \
                 (apply+check, StartApp excluded)",
                self.steps
            );
        }
    }
}

// ---------------------------------------------------------------------------
// Headless stepper — the proptest-macro path. A generic adapter over ANY
// `StateMachineTest` whose reference is the shared (`ReferenceState`,
// `E2ETransition`) machine. It delegates each phase to the test type's own
// `init_test` / `apply` / `check_invariants`, so a slice keeps its own
// invariant set and capture-on-panic `Drop` — only the *loop* is now shared
// via `run_sequence`. `E2ESut` itself satisfies the bound, so this also covers
// the GPUI-replay headless path (`SmtStepper<E2ESut>`).
// ---------------------------------------------------------------------------

use proptest_state_machine::StateMachineTest;

/// Bound shared by every PBT slice's generated machine: a `StateMachineTest`
/// over the one backend-blind reference machine.
pub trait HeadlessTest: StateMachineTest<Reference = Self::Ref> + Sized + 'static {
    type Ref: ReferenceStateMachine<State = ReferenceState, Transition = E2ETransition>;
}

impl<T> HeadlessTest for T
where
    T: StateMachineTest + 'static,
    T::Reference: ReferenceStateMachine<State = ReferenceState, Transition = E2ETransition>,
{
    type Ref = T::Reference;
}

pub struct SmtStepper<T: HeadlessTest> {
    sut: Option<T::SystemUnderTest>,
}

impl<T: HeadlessTest> Default for SmtStepper<T> {
    fn default() -> Self {
        Self { sut: None }
    }
}

impl<T: HeadlessTest> Stepper for SmtStepper<T> {
    fn init(&mut self, ref_state: &ReferenceState) {
        // Builds the wiring-selected SUT and resets the capture buffer — both
        // live in the slice's own `init_test`. On a later panic, dropping
        // `self.sut` runs the wrapper's `Drop`, which writes the JSON capture.
        self.sut = Some(T::init_test(ref_state));
    }

    fn apply(&mut self, ref_state: &ReferenceState, transition: &E2ETransition) {
        // `StateMachineTest::apply` is by-value; thread the owned SUT through.
        let sut = self.sut.take().expect("init() runs before apply()");
        self.sut = Some(T::apply(sut, ref_state, transition.clone()));
    }

    fn check_invariants(&mut self, ref_state: &ReferenceState) {
        T::check_invariants(
            self.sut.as_ref().expect("init() runs before check"),
            ref_state,
        );
    }

    /// Route the engine's end-of-case hook to the slice's own
    /// `StateMachineTest::teardown` (by-value, like the library loop would).
    fn finish(&mut self, ref_state: &ReferenceState) {
        let sut = self.sut.take().expect("init() runs before finish()");
        T::teardown(sut, ref_state.clone());
    }
}

/// Drop-in replacement for the library's default `StateMachineTest::test_sequential`:
/// runs the slice through the shared [`run_sequence`] engine in [`ReplayMode::Strict`]
/// (behaviour-identical to the library loop — no gating skips during generation).
/// Each generated `StateMachineTest` impl overrides `test_sequential` to call this.
pub fn run_via_state_machine_test<T: HeadlessTest>(
    ref_state: ReferenceState,
    transitions: Vec<E2ETransition>,
    seen_counter: Option<Arc<AtomicUsize>>,
) {
    let mut stepper = SmtStepper::<T>::default();
    run_sequence(
        &mut stepper,
        ref_state,
        transitions,
        seen_counter,
        ReplayMode::Strict,
    );
    // `stepper` (hence the SUT) drops here on the happy path; on a panic it
    // unwinds the same way, so the capture `Drop` fires identically.
}

// ---------------------------------------------------------------------------
// GPUI live-SUT stepper — the REPLAY path (bisection / re-checking a headless
// capture on a real window). NOT the live random generator.
//
// Why not the generator: reading the real harness (phased.rs
// `run_pbt_with_driver_sync_callback`) showed the live random loop cannot and
// should not collapse into `run_sequence`:
//   - the window is launched *mid-sequence* (between the pre-startup and
//     post-startup loops), not before it;
//   - generation is incremental and seed-reproducible — pre-generating a `Vec`
//     would change the `PROPTEST_SEED` → sequence mapping and break repro;
//   - each post-startup step wraps apply+check in real-input gestures
//     (`driver.try_ui_interaction`), a CDC sync wait, per-step screenshots, and
//     a `catch_unwind`-with-Post-screenshot — none of which fit a plain loop.
// The "main-thread bridge" sketched in the first draft was also wrong: the
// driver lives *inside* the SUT and already owns the window round-trip.
//
// Where the Stepper seam DOES fit GPUI is REPLAY: a fixed `Vec<E2ETransition>`
// (a JSON capture or a bisection candidate) driven through an already-launched
// window. Then there is no incremental generation, no mid-sequence launch, and
// `run_sequence(.., SkipGated)` is exactly right. This stepper borrows the live
// SUT + driver the harness already built; per step it mirrors the post-startup
// body's two branches (gesture for UI ops, backend apply otherwise).
// ---------------------------------------------------------------------------

pub struct GpuiReplayStepper<'a> {
    runtime: &'a tokio::runtime::Runtime,
    sut: &'a mut E2ESut,
    driver: &'a mut dyn crate::UiDriver,
}

impl<'a> GpuiReplayStepper<'a> {
    /// Borrow the live SUT + driver the harness has already wired to the window.
    pub fn new(
        runtime: &'a tokio::runtime::Runtime,
        sut: &'a mut E2ESut,
        driver: &'a mut dyn crate::UiDriver,
    ) -> Self {
        Self {
            runtime,
            sut,
            driver,
        }
    }
}

impl Stepper for GpuiReplayStepper<'_> {
    fn init(&mut self, _: &ReferenceState) {
        // No-op: the SUT and window are injected (built once by the harness and
        // shared across the launch boundary), not constructed per case.
    }

    fn apply(&mut self, ref_state: &ReferenceState, transition: &E2ETransition) {
        // Mirror the post-startup body (phased.rs `run_step_body_with_post_overlay`),
        // minus screenshots: a user-facing op is driven as a real gesture through
        // the window; anything else is a backend apply, exactly like headless.
        match crate::pbt::phased::resolve_ui_operation(transition, self.sut) {
            Some((entity, op, params)) => self.runtime.block_on(async {
                let handled = self.driver.try_ui_interaction(&entity, &op, &params).await;
                if !handled {
                    let drv = self
                        .sut
                        .driver
                        .borrow()
                        .clone()
                        .expect("UserDriver not installed");
                    drv.synthetic_dispatch(&entity, &op, params.clone())
                        .await
                        .expect("synthetic_dispatch failed during replay");
                }
                self.driver.settle().await;
                let expected = self.sut.expected_block_ids(ref_state);
                self.sut
                    .wait_for_blocks_synced(&expected, std::time::Duration::from_millis(10_000))
                    .await;
            }),
            None => self.sut.drive_transition(ref_state, transition),
        }
    }

    fn check_invariants(&mut self, ref_state: &ReferenceState) {
        let runtime = self.runtime;
        runtime.block_on(self.sut.run_invariant_registry(ref_state));
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
// hard-wired to `StorageSelector::Turso` (it calls `E2ESut::new`), so it ignores
// the node's wiring. `BisectionStepper` instead builds the SUT with the storage
// substrate the node's wiring implies (`storage_selector_for_wiring`, exactly as
// the `declare_pbt_slice!` macro does), and runs the wiring-selected invariant
// registry. The same captured `Vec<E2ETransition>` can then replay against every
// node via `run_sequence(.., SkipGated)`: transitions the node gates out become
// `SkippedByGating` no-ops; everything else applies and is checked.
// ---------------------------------------------------------------------------

pub struct BisectionStepper {
    runtime: Arc<tokio::runtime::Runtime>,
    sut: Option<E2ESut>,
}

impl Default for BisectionStepper {
    fn default() -> Self {
        // One process-wide runtime shared across every lattice node, mirroring
        // `E2ESut::init_test`'s `SHARED_RUNTIME`. Per-node isolation comes from
        // the SUT's own state (TempDir, DB, session) dropping at `init`.
        static SHARED_RUNTIME: std::sync::OnceLock<Arc<tokio::runtime::Runtime>> =
            std::sync::OnceLock::new();
        let runtime = SHARED_RUNTIME
            .get_or_init(|| Arc::new(tokio::runtime::Runtime::new().unwrap()))
            .clone();
        Self { runtime, sut: None }
    }
}

impl Stepper for BisectionStepper {
    fn init(&mut self, ref_state: &ReferenceState) {
        // Build the SUT for *this node's* wiring — the whole point of bisection.
        // Dropping any previous SUT here releases its DB/session Arcs.
        let storage = crate::pbt::storage_selector_for_wiring(&ref_state.wiring);
        self.sut = Some(
            E2ESut::new_with_backend(self.runtime.clone(), storage)
                .expect("BisectionStepper: SUT construction failed"),
        );
    }

    fn apply(&mut self, ref_state: &ReferenceState, transition: &E2ETransition) {
        let sut = self.sut.as_mut().expect("init() runs before apply()");
        sut.drive_transition(ref_state, transition);
    }

    fn check_invariants(&mut self, ref_state: &ReferenceState) {
        let sut = self.sut.as_ref().expect("init() runs before check()");
        self.runtime.block_on(sut.run_invariant_registry(ref_state));
    }
}

// ---------------------------------------------------------------------------
// Integration points:
//
// 1. Proptest-macro path (slice.rs). DONE — both `__declare_pbt_slice_wrapper!`
//    and `__declare_pbt_full_slice!` override `StateMachineTest::test_sequential`
//    to call `run_via_state_machine_test::<Self>(..)`, so every blessed slice's
//    per-case loop now runs through `run_sequence`. Generation, shrinking, and
//    `.proptest-regressions` replay are unchanged (they live in
//    `sequential_strategy`, above the loop).
//
// 2. Live GPUI generator (phased.rs `run_pbt_with_driver_sync_callback`).
//    INTENTIONALLY left hand-rolled — see the block comment above. It already
//    shares the per-step *primitives* with headless (`E2ESut::drive_transition`,
//    `apply_transition_async`, `run_invariant_registry`); the loop shell stays
//    GPUI-specific (mid-sequence window launch + per-step screenshots + seed
//    repro).
//
// 3. GPUI replay + bisection (ADR 0009 §3/§5). `GpuiReplayStepper` re-checks a
//    captured `Vec<E2ETransition>` on a launched window:
//        let mut s = GpuiReplayStepper::new(&runtime, &mut sut, driver);
//        run_sequence(&mut s, ref0, captured, None, ReplayMode::SkipGated);
//    The same capture replays across a ComponentSet lattice (headless nodes via
//    `SmtStepper`, the UI node via `GpuiReplayStepper`) to localise the smallest
//    reproducing set.
// ---------------------------------------------------------------------------

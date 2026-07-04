//! Hand-authored and machine-captured fixtures, replayed through the same
//! SUT + reference-state machine + invariants.
//!
//! Two `FixtureSource`s feed one strict runner:
//! - [`json::JsonFixtureSource`] — machine-captured shrunk counterexamples
//!   (`*.json`), one fixture per file.
//! - [`gherkin::GherkinFixtureSource`] — hand-authored `*.feature` files, one
//!   fixture per scenario.
//!
//! Strict semantics: a failed precondition is a **hard panic** (no silent
//! skip), and the SUT's invariants run after every step via
//! `S::check_invariants`. A fixture encoding a stale assumption fails loud.
//!
//! `FixtureSource` unifies JSON + Gherkin behind one strict runner
//! ([`replay_steps`]), replacing the old lenient `slice::run_fixture_dir`. The
//! same runner backs the headless slice tests and the real-GPUI replay binary;
//! they differ only in which SUT/driver is plugged in.

pub mod assert;
pub mod assert_steps;
pub mod gherkin;
pub mod json;
pub mod matchers;

use std::path::Path;

use assert::Assertion;
use holon_pbt_core::capabilities::RefLifecycle;
use proptest::strategy::Strategy;
use proptest::strategy::ValueTree;
use proptest::test_runner::TestRunner;
use proptest_state_machine::ReferenceStateMachine;
use proptest_state_machine::StateMachineTest;

use crate::pbt::transitions::E2ETransition;

/// One replayable step: a `Given`/`When` action or a `Then` assertion.
pub enum FixtureStep {
    Action(E2ETransition),
    Assert(Assertion),
}

/// Bridge that lets the generic runner evaluate an [`Assertion`] against the
/// SUT's capability traits. `ComposedSut` implements it by dispatching through
/// its `CapMap` (`evaluate_assertion_caps`).
pub trait FixtureAssertable {
    fn evaluate_assert(
        &self,
        assertion: &Assertion,
        ref_state: &crate::pbt::ReferenceState,
    ) -> Result<(), String>;
}

/// A named, self-contained transition sequence to replay from a fresh init.
pub struct NamedFixture {
    pub name: String,
    pub description: String,
    /// Wiring the sequence was recorded under (`Fixture.environment.wiring`).
    /// `None` for hand-authored sources (Gherkin, pre-header captures) —
    /// replayers fall back to `Wiring::full()`.
    pub wiring: Option<holon_pbt_core::Wiring>,
    /// Editor-shape env flags recorded at capture time
    /// (`Fixture.environment.env_flags`, keys from
    /// `holon_pbt_core::fixture::CAPTURE_ENV_FLAGS`). Replaying under
    /// different flags changes the transition alphabet — preconditions
    /// like `LoroRequiredForAtomicEditor` consult them — so out-of-process
    /// replayers (the windowed minimizer) must apply these before stepping.
    /// Empty for hand-authored sources.
    pub env_flags: std::collections::BTreeMap<String, String>,
    pub steps: Vec<FixtureStep>,
}

impl NamedFixture {
    /// Apply the recorded editor-shape flags to the process env so replay
    /// preconditions see the same transition alphabet the capture was
    /// generated under. Recognized flags absent from the capture are
    /// REMOVED (recorded-as-unset). Only for out-of-process replayers
    /// (windowed minimizer, capture replay) — in-process shrinkers already
    /// run under the generating env, and Gherkin fixtures record no flags.
    pub fn apply_recorded_env_flags(&self) {
        for key in holon_pbt_core::fixture::CAPTURE_ENV_FLAGS {
            match self.env_flags.get(*key) {
                Some(v) => unsafe { std::env::set_var(key, v) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        eprintln!(
            "[fixture {}] applied recorded env flags: {:?}",
            self.name, self.env_flags
        );
    }
}

/// A provider of [`NamedFixture`]s (JSON files, Gherkin scenarios, …).
///
/// `load` fails loud (panics) on a parse/IO error rather than returning a
/// `Result` — a malformed fixture must not silently drop out of the run.
pub trait FixtureSource {
    /// Short label for diagnostics, e.g. `"json"` / `"gherkin"`.
    fn kind(&self) -> &'static str;
    fn load(&self) -> Vec<NamedFixture>;
}

/// Replay every fixture from every source through the SUT machine with strict
/// semantics. A missing source dir yields zero fixtures (no-op) — a slice may
/// declare a fixtures dir before any fixtures exist.
pub fn run_fixtures<M, S>(sources: &[Box<dyn FixtureSource>])
where
    M: ReferenceStateMachine<State = crate::pbt::ReferenceState, Transition = E2ETransition>,
    S: StateMachineTest<SystemUnderTest = S, Reference = M> + FixtureAssertable,
{
    // Pin the primary's Loro peer id below the SUT's peer ids (peer_idx+100)
    // so concurrent same-position inserts tie-break the same way as the
    // oracle's shadow mesh (shadow primary=1 < shadow peers=100+idx). With a
    // random primary id the merge interleaving flips per run — the axis-3
    // conflict fixture caught exactly that.
    crate::pbt::set_loro_peer_id_if_unset("1");
    for source in sources {
        let kind = source.kind();
        for fixture in source.load() {
            replay_one::<M, S>(kind, &fixture);
        }
    }
}

/// Replay a single feature file (used by the Phase-1 proof-point tests).
/// Asserts the file yields at least one scenario.
pub fn run_feature_strict<M, S>(path: impl AsRef<Path>)
where
    M: ReferenceStateMachine<State = crate::pbt::ReferenceState, Transition = E2ETransition>,
    S: StateMachineTest<SystemUnderTest = S, Reference = M> + FixtureAssertable,
{
    let path = path.as_ref();
    let fixtures = gherkin::GherkinFixtureSource::new(path).load();
    assert!(
        !fixtures.is_empty(),
        "[gherkin] {} contained no scenarios",
        path.display()
    );
    for fixture in &fixtures {
        replay_one::<M, S>("gherkin", fixture);
    }
}

fn replay_one<M, S>(kind: &str, fixture: &NamedFixture)
where
    M: ReferenceStateMachine<State = crate::pbt::ReferenceState, Transition = E2ETransition>,
    S: StateMachineTest<SystemUnderTest = S, Reference = M> + FixtureAssertable,
{
    eprintln!(
        "[fixtures:{kind}] replaying {:?} ({} steps)",
        fixture.name,
        fixture.steps.len()
    );

    let mut runner = TestRunner::deterministic();
    let init_tree = M::init_state()
        .new_tree(&mut runner)
        .unwrap_or_else(|_| panic!("[fixtures:{kind}] init_state failed for {:?}", fixture.name));
    let ref_state = init_tree.current();
    let sut = S::init_test(&ref_state);

    let label = format!("fixtures:{kind} {:?}", fixture.name);
    // Headless replay needs no post-StartApp setup; the GPUI path passes a
    // hook here to launch a window + swap in a real driver.
    let _sut = replay_steps::<M, S>(
        &label,
        &fixture.steps,
        ref_state,
        sut,
        |_| {},
        |_, _| {},
        None,
    );

    eprintln!("[fixtures:{kind}] {:?} OK", fixture.name);
}

/// The medium-agnostic replay core: drive `steps` through an already-created
/// SUT + reference state with strict semantics (precondition / vacuity / assert
/// failures panic). Shared by the headless slice runner and the real-GPUI
/// replay binary — the only difference is which `UserDriver` the SUT has and
/// the `after_start_app` hook (a no-op headlessly; window + driver injection
/// for GPUI). Returns the SUT so the caller can tear it down.
///
/// `after_start_app` fires once, immediately after the StartApp transition's
/// invariants pass (the point at which the `ReactiveEngine` exists).
pub fn replay_steps<M, S>(
    label: &str,
    steps: &[FixtureStep],
    mut ref_state: M::State,
    mut sut: S,
    mut after_start_app: impl FnMut(&mut S),
    // Per-tick hook fired after EVERY post-StartApp transition's invariants pass,
    // with both the live SUT and reference state in scope (B-track E4: the windowed
    // sim builds a windowed `CapMap` + ref from `(sut, ref_state)` and runs the
    // composed catalog over it each tick). A no-op for the headless slices. Distinct
    // from `after_start_app`, which fires ONCE. The pre-StartApp ticks are skipped
    // (no rendered state to check) by gating on `ref_state.app_started()`.
    mut per_tick: impl FnMut(&mut S, &M::State),
    seen_counter: Option<std::sync::Arc<std::sync::atomic::AtomicUsize>>,
) -> S
where
    M: ReferenceStateMachine<State = crate::pbt::ReferenceState, Transition = E2ETransition>,
    S: StateMachineTest<SystemUnderTest = S, Reference = M> + FixtureAssertable,
{
    let mut timing = crate::pbt::stepper::StepTimingAgg::default();
    for (i, step) in steps.iter().enumerate() {
        match step {
            FixtureStep::Action(t) => {
                assert!(
                    M::preconditions(&ref_state, t),
                    "[{label}] step {i}: preconditions FAILED for {} — the fixture encodes a \
                     stale assumption",
                    t.variant_name()
                );
                // Mark this transition seen BEFORE applying it (matching
                // proptest-state-machine's `test_sequential`): the proptest
                // shrinker's first step deletes never-seen trailing transitions,
                // so the *failing* transition must be counted before it panics or
                // the shrinker drops the very transition that reproduces. `None`
                // outside the windowed proptest path (the library leaves it
                // `None` during shrink runs too).
                if let Some(counter) = seen_counter.as_ref() {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                }
                let is_start_app = t.variant_name() == "StartApp";
                ref_state = M::apply(ref_state, t);
                let t_apply = std::time::Instant::now();
                sut = S::apply(sut, &ref_state, t.clone());
                let apply_ms = t_apply.elapsed().as_millis();
                let t_check = std::time::Instant::now();
                S::check_invariants(&sut, &ref_state);
                let check_ms = t_check.elapsed().as_millis();
                timing.record(t.variant_name(), apply_ms, check_ms);
                if crate::pbt::stepper::step_timing_enabled() {
                    eprintln!(
                        "[step_timing] step={} {} apply_ms={apply_ms} check_ms={check_ms}",
                        i + 1,
                        t.variant_name(),
                    );
                }
                if is_start_app {
                    after_start_app(&mut sut);
                }
                // Per-tick composed-catalog hook (E4). Fires only once the app is
                // started (a rendered window exists to check) and after `after_start_app`
                // has installed the driver/frontend on the StartApp tick.
                if ref_state.app_started() {
                    per_tick(&mut sut, &ref_state);
                }
                // Fault injection: a deterministic forced failure after
                // the 1-based step `HOLON_PBT_FORCE_FAIL_AT_STEP`, so the windowed
                // proptest shrinker + capture pipeline can be exercised without a
                // flaky real seed. The message carries the `format_layer_report`
                // marker so the shrinker's default signature recognizes it; the
                // shrinker reduces the sequence down to this step. No effect unless
                // the env var is set.
                if let Ok(n) = std::env::var("HOLON_PBT_FORCE_FAIL_AT_STEP") {
                    if n.parse::<usize>().ok() == Some(i + 1) {
                        panic!(
                            "HOLON_PBT_FORCE_FAIL_AT_STEP={n}: forced failure after step {} ({}) \
                             — trouble begins at: forced-fault",
                            i + 1,
                            t.variant_name()
                        );
                    }
                }
            }
            FixtureStep::Assert(assertion) => {
                // Reject a `Then` before startup as vacuous: there is no
                // rendered state to assert against.
                assert!(
                    ref_state.app_started(),
                    "[{label}] step {i}: assertion before app startup is vacuous — put `Then` \
                     steps after the app is started",
                );
                if let Err(msg) = sut.evaluate_assert(assertion, &ref_state) {
                    panic!("[{label}] step {i}: {msg}");
                }
            }
        }
    }
    timing.finish(label);
    sut
}

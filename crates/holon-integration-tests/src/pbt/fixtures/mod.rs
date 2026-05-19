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
pub mod gherkin;
pub mod json;
pub mod matchers;

use std::path::Path;

use holon_pbt_core::capabilities::RefLifecycle;
use proptest::strategy::{Strategy, ValueTree};
use proptest::test_runner::TestRunner;
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};

use crate::pbt::transitions::E2ETransition;
use assert::Assertion;

/// One replayable step: a `Given`/`When` action or a `Then` assertion.
pub enum FixtureStep {
    Action(E2ETransition),
    Assert(Assertion),
}

/// Bridge that lets the generic runner evaluate an [`Assertion`] against the
/// SUT's capability traits. `E2ESut` implements it directly (it owns the
/// capabilities); the `declare_pbt_slice!` wrapper SUT delegates to its inner
/// `E2ESut`.
pub trait FixtureAssertable {
    fn evaluate_assert(
        &self,
        assertion: &Assertion,
        ref_state: &crate::pbt::ReferenceState,
    ) -> Result<(), String>;
}

/// The inner `E2ESut` carries the capability impls, so it evaluates assertions
/// directly. The `declare_pbt_slice!` wrapper SUT delegates here. The real-GPUI
/// replay drives `E2ESut<Full>` as `S` directly, so it uses this impl too.
impl<V: crate::pbt::VariantMarker> FixtureAssertable for crate::pbt::E2ESut<V> {
    fn evaluate_assert(
        &self,
        assertion: &Assertion,
        ref_state: &crate::pbt::ReferenceState,
    ) -> Result<(), String> {
        self.runtime
            .block_on(assert::evaluate_assertion(assertion, ref_state, self))
    }
}

/// A named, self-contained transition sequence to replay from a fresh init.
pub struct NamedFixture {
    pub name: String,
    pub description: String,
    pub steps: Vec<FixtureStep>,
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
pub fn run_fixtures<M, S, V>(sources: &[Box<dyn FixtureSource>])
where
    V: crate::pbt::VariantMarker,
    M: ReferenceStateMachine<State = crate::pbt::VariantRef<V>, Transition = E2ETransition>,
    S: StateMachineTest<SystemUnderTest = S, Reference = M> + FixtureAssertable,
{
    for source in sources {
        let kind = source.kind();
        for fixture in source.load() {
            replay_one::<M, S, V>(kind, &fixture);
        }
    }
}

/// Replay a single feature file (used by the Phase-1 proof-point tests).
/// Asserts the file yields at least one scenario.
pub fn run_feature_strict<M, S, V>(path: impl AsRef<Path>)
where
    V: crate::pbt::VariantMarker,
    M: ReferenceStateMachine<State = crate::pbt::VariantRef<V>, Transition = E2ETransition>,
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
        replay_one::<M, S, V>("gherkin", fixture);
    }
}

fn replay_one<M, S, V>(kind: &str, fixture: &NamedFixture)
where
    V: crate::pbt::VariantMarker,
    M: ReferenceStateMachine<State = crate::pbt::VariantRef<V>, Transition = E2ETransition>,
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
    let _sut = replay_steps::<M, S, V>(&label, &fixture.steps, ref_state, sut, |_| {});

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
pub fn replay_steps<M, S, V>(
    label: &str,
    steps: &[FixtureStep],
    mut ref_state: M::State,
    mut sut: S,
    mut after_start_app: impl FnMut(&mut S),
) -> S
where
    V: crate::pbt::VariantMarker,
    M: ReferenceStateMachine<State = crate::pbt::VariantRef<V>, Transition = E2ETransition>,
    S: StateMachineTest<SystemUnderTest = S, Reference = M> + FixtureAssertable,
{
    for (i, step) in steps.iter().enumerate() {
        match step {
            FixtureStep::Action(t) => {
                assert!(
                    M::preconditions(&ref_state, t),
                    "[{label}] step {i}: preconditions FAILED for {} \
                     — the fixture encodes a stale assumption",
                    t.variant_name()
                );
                let is_start_app = t.variant_name() == "StartApp";
                ref_state = M::apply(ref_state, t);
                sut = S::apply(sut, &ref_state, t.clone());
                S::check_invariants(&sut, &ref_state);
                if is_start_app {
                    after_start_app(&mut sut);
                }
            }
            FixtureStep::Assert(assertion) => {
                // Reject a `Then` before startup as vacuous: there is no
                // rendered state to assert against.
                assert!(
                    ref_state.app_started(),
                    "[{label}] step {i}: assertion before app startup is vacuous \
                     — put `Then` steps after the app is started",
                );
                if let Err(msg) = sut.evaluate_assert(assertion, &ref_state) {
                    panic!("[{label}] step {i}: {msg}");
                }
            }
        }
    }
    sut
}

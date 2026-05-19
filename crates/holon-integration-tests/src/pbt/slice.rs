//! `declare_pbt_slice!` — declarative shortcut for new PBT slices.
//!
//! See [`docs/Testing/PbtSlicing.md`](../../../../docs/Testing/PbtSlicing.md).
//! Replaces the speculative `PbtSlice` trait from §5 (typed-tuple
//! `TransitionSet`/`InvariantSet`) with a macro that accepts arbitrary
//! expressions per item — required to express slice-specific quirks like
//! `prop_filter` on `WriteOrgFile` and `PhantomData<R>`-carrying invariants.
//!
//! ## Capture-on-panic + fixture-dir replay
//!
//! Every slice declared via `declare_pbt_slice!` automatically captures
//! the transition sequence it just applied; if the proptest assertion
//! panic unwinds through the slice's SUT wrapper, the captured sequence
//! is written to `tests/.captures/<test_fn>.captured.json`. Hand-edit /
//! rename it into `tests/fixtures/<test_fn>/<name>.json` (or wherever
//! the slice's `fixtures_dir` points) to lock it as a literal-value
//! regression.
//!
//! When `fixtures_dir: "..."` is supplied to the macro, an additional
//! test function (`fixtures_test_fn: <ident>`) scans that directory and
//! replays every `*.json` fixture before the proptest sweep runs.

/// Internal helper: emit one arm of the transition union. Dispatches on
/// shape: plain type path → no filter; paren-wrapped 3-tuple `(Type,
/// "reason", filter)` → filtered. Macro must be at root visibility because
/// `$crate` substitution applies at the call site.
#[doc(hidden)]
#[macro_export]
macro_rules! __declare_pbt_slice_arm {
    // Filtered: paren-wrapped (Type, "reason", filter_closure).
    ($arms:ident, $state:expr, ($ty:path, $reason:expr, $filter:expr $(,)?)) => {{
        use ::proptest::strategy::Strategy;
        if let ::validated::Validated::Good((w, s)) =
            <$ty as $crate::pbt::transition_dispatch::E2ETransitionFactory>::weighted_generator(
                $state,
            )
        {
            let filtered = s
                .prop_filter($reason, $filter)
                .prop_map($crate::pbt::transitions::E2ETransition::from)
                .boxed();
            $arms.push((w, filtered));
        }
    }};
    // Plain: bare type path, no filter.
    ($arms:ident, $state:expr, $ty:path) => {{
        use ::proptest::strategy::Strategy;
        if let ::validated::Validated::Good((w, s)) =
            <$ty as $crate::pbt::transition_dispatch::E2ETransitionFactory>::weighted_generator(
                $state,
            )
        {
            $arms.push((
                w,
                s.prop_map($crate::pbt::transitions::E2ETransition::from)
                    .boxed(),
            ));
        }
    }};
}

/// Declare a PBT slice with the minimum boilerplate.
///
/// ```ignore
/// declare_pbt_slice! {
///     test_fn: my_slice_pbt,
///     machine: MySliceMachine,
///     sut_wrapper: MySliceSut,
///     variant_ref: $crate::pbt::VariantRef<$crate::pbt::SqlOnly>,
///     inner_sut: $crate::pbt::E2ESut<$crate::pbt::SqlOnly>,
///     transitions: [
///         StartApp,
///         // Filtered form: (Type, "reason", |t: &Type| keep_predicate)
///         (WriteOrgFile, "skip index.org (CDC quiescence race)",
///             |t: &WriteOrgFile| t.filename != "index.org"),
///         BulkExternalAdd,
///     ],
///     invariants: [
///         InvLoroNoErrors,
///         InvBlockTagsReferencesExist(::std::marker::PhantomData),
///     ],
///     cases: 16,
///     max_shrink_iters: 20,
///     steps: 1..10,
/// }
/// ```
///
/// Each `transitions:` entry is either a bare type path (no filter) or a
/// paren-wrapped `(Type, "reason", filter_closure)` tuple — the filter is
/// applied via `proptest::strategy::Strategy::prop_filter`. Each
/// `invariants:` entry is an expression that constructs an
/// `Invariant<R, S>` implementor. The macro generates:
///
/// - a `ReferenceStateMachine` impl on `$machine`,
/// - a `StateMachineTest` impl on `$sut_wrapper` whose `check_invariants`
///   iterates the listed invariants and panics on `Fail`,
/// - the `prop_state_machine!` entry point named `$test_fn`.
#[macro_export]
macro_rules! declare_pbt_slice {
    (
        test_fn: $test_fn:ident,
        machine: $machine:ident,
        sut_wrapper: $sut_wrapper:ident,
        variant_ref: $variant_ref:ty,
        inner_sut: $inner_sut:ty,
        transitions: [ $( $transition:tt ),* $(,)? ],
        invariants: [ $( $invariant_expr:expr ),* $(,)? ],
        cases: $cases:expr,
        max_shrink_iters: $max_shrink_iters:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(, fixtures_test_fn: $fixtures_test_fn:ident, fixtures_dir: $fixtures_dir:literal)?
        $(,)?
    ) => {
        pub struct $machine;

        impl ::proptest_state_machine::ReferenceStateMachine for $machine {
            type State = $variant_ref;
            type Transition = $crate::pbt::transitions::E2ETransition;

            fn init_state() -> ::proptest::strategy::BoxedStrategy<Self::State> {
                <$variant_ref as ::proptest_state_machine::ReferenceStateMachine>::init_state()
            }

            fn transitions(state: &Self::State) -> ::proptest::strategy::BoxedStrategy<Self::Transition> {
                use ::proptest::strategy::{Strategy, Union};
                use $crate::pbt::transition_dispatch::E2ETransitionFactory;
                use $crate::pbt::transitions::E2ETransition;
                let mut arms: Vec<(u32, ::proptest::strategy::BoxedStrategy<E2ETransition>)>
                    = Vec::new();
                $(
                    $crate::__declare_pbt_slice_arm!(arms, &**state, $transition);
                )*
                assert!(
                    !arms.is_empty(),
                    concat!(stringify!($test_fn), ": no transition applicable")
                );
                Union::new_weighted(arms).boxed()
            }

            fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
                use $crate::pbt::transitions::E2ETransitionImpl;
                use $crate::pbt::validation::record_rejection;
                use ::validated::Validated;
                match transition.preconditions(&**state) {
                    Validated::Good(()) => true,
                    Validated::Fail(reasons) => {
                        record_rejection(transition.variant_name(), &reasons);
                        false
                    }
                }
            }

            fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
                use $crate::pbt::transitions::E2ETransitionImpl;
                transition.apply_to_ref(&mut *state);
                state.last_transition_kind = Some(transition.variant_name());
                state
            }
        }

        pub struct $sut_wrapper {
            inner: $inner_sut,
        }

        impl ::std::ops::Drop for $sut_wrapper {
            fn drop(&mut self) {
                // When proptest finds a failing case and the assertion
                // panic unwinds through our wrapper, write the captured
                // transition sequence to a JSON fixture next to the test
                // file. Seed-based regression files are invalidated by
                // any strategy change; the fixture is a literal-value
                // replay (cf. `holon-pbt-core::fixture`).
                if ::std::thread::panicking() {
                    $crate::pbt::slice::write_captured_fixture(stringify!($test_fn));
                }
            }
        }

        impl ::proptest_state_machine::StateMachineTest for $sut_wrapper {
            type SystemUnderTest = Self;
            type Reference = $machine;

            fn init_test(
                _: &<Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::State,
            ) -> Self::SystemUnderTest {
                static SHARED_RUNTIME: ::std::sync::OnceLock<
                    ::std::sync::Arc<::tokio::runtime::Runtime>,
                > = ::std::sync::OnceLock::new();
                let runtime = SHARED_RUNTIME
                    .get_or_init(|| {
                        ::std::sync::Arc::new(::tokio::runtime::Runtime::new().unwrap())
                    })
                    .clone();
                $crate::pbt::slice::reset_capture(stringify!($test_fn));
                $sut_wrapper {
                    inner: <$inner_sut>::new(runtime).unwrap(),
                }
            }

            fn apply(
                mut sut: Self::SystemUnderTest,
                ref_state:
                    &<Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::State,
                transition:
                    <Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::Transition,
            ) -> Self::SystemUnderTest {
                let runtime = sut.inner.runtime.clone();
                $crate::pbt::slice::record_transition(&transition);
                runtime.block_on(sut.inner.apply_transition_async(ref_state, &transition));
                sut
            }

            fn check_invariants(
                sut: &Self::SystemUnderTest,
                ref_state:
                    &<Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::State,
            ) {
                use ::holon_pbt_core::capabilities::RefLifecycle;
                if !ref_state.app_started() {
                    return;
                }
                let runtime = sut.inner.runtime.clone();
                let ref_inner: &$crate::pbt::ReferenceState = &**ref_state;
                runtime.block_on(async {
                    $(
                        {
                            use ::holon_pbt_core::invariant::{Invariant, InvariantResult};
                            match Invariant::check(&$invariant_expr, ref_inner, &sut.inner).await {
                                InvariantResult::Ok => {}
                                InvariantResult::Skipped(_) => {}
                                InvariantResult::Fail(msg) => panic!("{msg}"),
                            }
                        }
                    )*
                });
            }
        }

        ::proptest_state_machine::prop_state_machine! {
            #![proptest_config(::proptest::test_runner::Config {
                cases: $cases,
                max_shrink_iters: $max_shrink_iters,
                ..::proptest::test_runner::Config::default()
            })]
            #[test]
            fn $test_fn(sequential $step_lo .. $step_hi => $sut_wrapper);
        }

        // Optional: replay every `.json` file under `fixtures_dir` before
        // the proptest sweep gets a chance to find new regressions. Each
        // file is `Fixture<E2ETransition>`; preconditions that no longer
        // hold log a SkippedPreconditions diagnostic, so stale fixtures
        // degrade into a clear message instead of a silent pass.
        $(
            #[test]
            fn $fixtures_test_fn() {
                $crate::pbt::slice::run_fixture_dir::<
                    $machine,
                    $sut_wrapper,
                >($fixtures_dir);
            }
        )?
    };
}

// ── Capture-on-panic and fixture-dir replay helpers ───────────────

use crate::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
use proptest::strategy::{Strategy, ValueTree};
use proptest_state_machine::{ReferenceStateMachine, StateMachineTest};
use std::cell::RefCell;
use std::path::PathBuf;

thread_local! {
    /// Per-test-thread buffer of transitions applied by the active slice.
    /// Reset in `init_test`; appended to in `apply`; serialized to a
    /// `.captured.json` file by the SUT wrapper's `Drop` when the thread
    /// is panicking.
    static CAPTURE: RefCell<Option<CaptureState>> = RefCell::default();
}

struct CaptureState {
    transitions: Vec<E2ETransition>,
    already_written: bool,
}

pub fn reset_capture(_: &'static str) {
    CAPTURE.with(|c| {
        *c.borrow_mut() = Some(CaptureState {
            transitions: Vec::new(),
            already_written: false,
        });
    });
}

pub fn record_transition(t: &E2ETransition) {
    CAPTURE.with(|c| {
        if let Some(state) = c.borrow_mut().as_mut() {
            state.transitions.push(t.clone());
        }
    });
}

/// Write the captured transition sequence to
/// `$CARGO_MANIFEST_DIR/tests/.captures/<slice>.captured.json`. Idempotent
/// per panic (proptest may unwind through multiple Drop sites during
/// shrink — first writer wins).
pub fn write_captured_fixture(slice_name: &'static str) {
    CAPTURE.with(|c| {
        let mut borrow = c.borrow_mut();
        let Some(state) = borrow.as_mut() else {
            return;
        };
        if state.already_written || state.transitions.is_empty() {
            return;
        }
        state.already_written = true;
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join(".captures");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!("[slice capture] failed to mkdir {dir:?}: {e}");
            return;
        }
        let path = dir.join(format!("{slice_name}.captured.json"));
        let fixture: Fixture<E2ETransition, ()> = Fixture {
            name: format!("{slice_name}_captured"),
            description: "Auto-captured from a panicking proptest case. \
                          Rename + move into the slice's fixtures_dir to lock it."
                .to_string(),
            initial_state: (),
            transitions: state.transitions.clone(),
        };
        if let Err(e) = fixture.save(&path) {
            eprintln!("[slice capture] failed to write {path:?}: {e}");
        } else {
            eprintln!(
                "[slice capture] wrote {} transitions to {}",
                fixture.transitions.len(),
                path.display()
            );
        }
    });
}

/// Replay every `*.json` fixture under `dir`. Missing dir is a no-op
/// (lets slices declare a fixture path before any fixtures exist).
/// Each fixture is replayed with a fresh init state + fresh SUT.
pub fn run_fixture_dir<M, S>(dir: &str)
where
    M: ReferenceStateMachine<
            State = crate::pbt::VariantRef<crate::pbt::SqlOnly>,
            Transition = E2ETransition,
        >,
    S: StateMachineTest<SystemUnderTest = S, Reference = M>,
{
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(dir);
    if !root.exists() {
        eprintln!("[fixtures] {root:?} does not exist — no fixtures to replay");
        return;
    }
    let entries = match std::fs::read_dir(&root) {
        Ok(e) => e,
        Err(e) => {
            eprintln!("[fixtures] cannot read {root:?}: {e}");
            return;
        }
    };
    let mut failures: Vec<String> = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        let fixture: Fixture<E2ETransition, ()> = match Fixture::load(&path) {
            Ok(f) => f,
            Err(e) => {
                failures.push(format!("[fixtures] load {path:?}: {e}"));
                continue;
            }
        };
        eprintln!(
            "[fixtures] replaying {} ({} transitions)",
            fixture.name,
            fixture.transitions.len()
        );
        // Construct a deterministic init state via a fixed-seed runner.
        // The init_state strategy is the same one proptest uses; this
        // gives the fixture a stable starting point. If init_state's
        // shape changes, fixtures may need re-capturing.
        let mut runner = proptest::test_runner::TestRunner::deterministic();
        let init_tree = match M::init_state().new_tree(&mut runner) {
            Ok(t) => t,
            Err(_) => {
                failures.push(format!("[fixtures] {}: init_state failed", fixture.name));
                continue;
            }
        };
        let mut ref_state = init_tree.current();
        let mut sut = S::init_test(&ref_state);
        let mut applied = 0_usize;
        let mut skipped = 0_usize;
        for (i, t) in fixture.transitions.iter().enumerate() {
            if !M::preconditions(&ref_state, t) {
                eprintln!(
                    "  step {i}: SKIP (preconditions failed for {})",
                    t.variant_name_or_debug()
                );
                skipped += 1;
                continue;
            }
            ref_state = M::apply(ref_state, t);
            sut = S::apply(sut, &ref_state, t.clone());
            S::check_invariants(&sut, &ref_state);
            applied += 1;
        }
        eprintln!(
            "  done: applied={applied} skipped={skipped} (total={})",
            fixture.transitions.len()
        );
    }
    if !failures.is_empty() {
        for f in &failures {
            eprintln!("{f}");
        }
        panic!("{} fixture(s) failed to load/run", failures.len());
    }
}

trait VariantNameExt {
    fn variant_name_or_debug(&self) -> String;
}

impl VariantNameExt for E2ETransition {
    fn variant_name_or_debug(&self) -> String {
        self.variant_name().to_string()
    }
}

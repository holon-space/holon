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

/// Internal: dispatch a preset *name* to a group of transition entries.
/// Each arm calls `__declare_pbt_slice_arm!` once per entry. Add a new
/// preset by adding an arm here; callers reference it as
/// `preset <name>` inside a `transitions:` list.
#[doc(hidden)]
#[macro_export]
macro_rules! __expand_transition_preset {
    // `lifecycle` — the app-lifecycle bootstrap. `StartApp` is required
    // before any slice can do meaningful work.
    (lifecycle, $arms:ident, $state:expr) => {
        $crate::__declare_pbt_slice_arm!($arms, $state, $crate::pbt::transitions::StartApp);
    };
    // `org_writes` — `WriteOrgFile` edits excluding `index.org`, which
    // holds the default queries we don't want a slice to clobber.
    (org_writes, $arms:ident, $state:expr) => {
        $crate::__declare_pbt_slice_arm!(
            $arms,
            $state,
            (
                $crate::pbt::transitions::WriteOrgFile,
                "skip index.org (holds default queries)",
                |t: &$crate::pbt::transitions::WriteOrgFile| t.filename != "index.org",
            )
        );
    };
    // Unknown preset: emit a clear error pointing at this macro.
    ($name:ident, $arms:ident, $state:expr) => {
        compile_error!(concat!(
            "unknown transition preset `",
            stringify!($name),
            "` (see __expand_transition_preset! to add one)"
        ));
    };
}

/// Internal: dispatch a preset *name* to a group of invariant
/// expressions. Each arm emits the same `Invariant::check` block
/// `declare_pbt_slice!` produces inline. Currently empty — add presets
/// when more than one slice repeats the same bundle.
#[doc(hidden)]
#[macro_export]
macro_rules! __expand_invariant_preset {
    // `storage` — the storage-layer baseline: no Loro errors + every
    // `block_tags` row references a live block.
    (storage, $ref_inner:ident, $sut:ident) => {
        $crate::__build_invariant_arms!(
            $ref_inner,
            $sut,
            $crate::pbt::invariants::bodies::loro_no_errors::InvLoroNoErrors,
            $crate::pbt::invariants::bodies::block_tags_references_exist::InvBlockTagsReferencesExist(
                ::std::marker::PhantomData::<$crate::pbt::ReferenceState>
            ),
        );
    };
    // Fallback: unknown preset name in an `invariants:` list.
    ($name:ident, $ref_inner:ident, $sut:ident) => {
        compile_error!(concat!(
            "unknown invariant preset `",
            stringify!($name),
            "` (see __expand_invariant_preset! to add one)"
        ));
    };
}

/// Internal TT-muncher: walk a `transitions:` token stream, peel one
/// entry off the front per recursion. Three entry shapes are recognised:
/// `preset <name>` (delegates to `__expand_transition_preset!`),
/// paren-wrapped `(Type, "reason", filter)` (filtered factory), and a
/// bare type path (plain factory). A trailing comma is allowed and
/// optional after every form.
#[doc(hidden)]
#[macro_export]
macro_rules! __build_transition_arms {
    // Terminal: nothing left to process.
    ($arms:ident, $state:expr $(,)?) => {};
    // Preset reference.
    ($arms:ident, $state:expr, preset $name:ident $(, $($rest:tt)*)?) => {
        $crate::__expand_transition_preset!($name, $arms, $state);
        $( $crate::__build_transition_arms!($arms, $state, $($rest)*); )?
    };
    // Filtered factory: (Type, "reason", filter_closure).
    ($arms:ident, $state:expr, ($ty:path, $reason:expr, $filter:expr $(,)?) $(, $($rest:tt)*)?) => {
        $crate::__declare_pbt_slice_arm!($arms, $state, ($ty, $reason, $filter));
        $( $crate::__build_transition_arms!($arms, $state, $($rest)*); )?
    };
    // Plain factory: bare type path.
    ($arms:ident, $state:expr, $ty:path $(, $($rest:tt)*)?) => {
        $crate::__declare_pbt_slice_arm!($arms, $state, $ty);
        $( $crate::__build_transition_arms!($arms, $state, $($rest)*); )?
    };
}

/// Internal TT-muncher for `invariants:`. Same shape as
/// `__build_transition_arms!` but the leaf is an `expr` (the invariant
/// constructor) and the emitted code is the `Invariant::check` block.
/// Must be invoked inside an `async` context.
#[doc(hidden)]
#[macro_export]
macro_rules! __build_invariant_arms {
    ($ref_inner:ident, $sut:ident $(,)?) => {};
    ($ref_inner:ident, $sut:ident, preset $name:ident $(, $($rest:tt)*)?) => {
        $crate::__expand_invariant_preset!($name, $ref_inner, $sut);
        $( $crate::__build_invariant_arms!($ref_inner, $sut, $($rest)*); )?
    };
    ($ref_inner:ident, $sut:ident, $invariant:expr $(, $($rest:tt)*)?) => {
        {
            use ::holon_pbt_core::invariant::{Invariant, InvariantResult};
            match Invariant::check(&$invariant, $ref_inner, $sut).await {
                InvariantResult::Ok => {}
                InvariantResult::Skipped(_) => {}
                InvariantResult::Fail(msg) => panic!("{msg}"),
            }
        }
        $( $crate::__build_invariant_arms!($ref_inner, $sut, $($rest)*); )?
    };
}

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
        if let ::validated::Validated::Good((w, s)) = <$ty as ::holon_pbt_core::TransitionFactory<
            $crate::pbt::reference_state::ReferenceState,
        >>::weighted_generator($state)
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
        if let ::validated::Validated::Good((w, s)) = <$ty as ::holon_pbt_core::TransitionFactory<
            $crate::pbt::reference_state::ReferenceState,
        >>::weighted_generator($state)
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
///     variant_ref: $crate::pbt::VariantRef<$crate::pbt::SqlOnly>,
///     inner_sut: $crate::pbt::E2ESut<$crate::pbt::SqlOnly>,
///     transitions: [
///         preset lifecycle,                     // StartApp bootstrap
///         preset org_writes,                    // WriteOrgFile, excluding index.org
///         BulkExternalAdd,                      // one-off addition
///         (Indent, "skip top-level", |t: &Indent| t.depth > 0),
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
/// The `Machine` / `Sut` types and the optional fixtures-replay test
/// function are derived from `test_fn` via `paste::paste!`:
/// `my_slice_pbt` → `MySlicePbtMachine`, `MySlicePbtSut`, and
/// `my_slice_pbt_fixtures` (when `fixtures_dir:` is supplied).
///
/// Each `transitions:` entry is one of:
/// - `preset <name>` — references a group defined in
///   [`__expand_transition_preset!`]; multiple presets compose by listing
///   them on consecutive lines and free-mixing with one-off entries.
/// - A bare type path (no filter) — plain factory.
/// - A paren-wrapped `(Type, "reason", filter_closure)` tuple — filtered
///   factory.
///
/// `invariants:` entries are either `preset <name>` (see
/// [`__expand_invariant_preset!`]) or an expression that constructs an
/// `Invariant<R, S>` implementor.
///
/// ## Config
///
/// Supply *either* `cases:` + `max_shrink_iters:` (the macro builds a
/// `ProptestConfig` from them) *or* `proptest_config:` with a full
/// `ProptestConfig` expression — useful when several variants share one
/// regressions file or a custom `failure_persistence`.
///
/// ## Native (full-coverage) mode
///
/// Omit `variant_ref:`, `transitions:`, and `invariants:` to run the
/// **native** state machine directly: the SUT's own `ReferenceStateMachine`
/// (`VariantRef<V>`) drives the full transition set and the SUT's own
/// `check_invariants` runs the full invariant suite. This is the right
/// mode for broad end-to-end sweeps (cf. `general_e2e_pbt`). No wrapper
/// types are generated and no capture-on-panic fixture is written, so the
/// native SUT's `apply` (otel spans, `last_transition`) is used verbatim.
/// `proptest_config:` is required; optional attributes (e.g.
/// `#[ignore = "..."]`) may precede `inner_sut:`:
///
/// ```ignore
/// declare_pbt_slice! {
///     test_fn: general_e2e_pbt,
///     inner_sut: $crate::pbt::E2ESut<$crate::pbt::Full>,
///     proptest_config: pbt_config(),
///     steps: 3..20,
/// }
/// ```
#[macro_export]
macro_rules! declare_pbt_slice {
    // ── Native (full-coverage) mode ─────────────────────────────────
    // No wrapper: run `prop_state_machine!` against the SUT directly,
    // reusing its native `ReferenceStateMachine` + `StateMachineTest`.
    // `inner_sut` is matched as an ident + optional generic args (not a
    // `:ty` fragment) because `prop_state_machine!` re-parses the SUT type
    // as `$test:ident $(<...>)?` — an opaque `:ty` fragment can't satisfy
    // that. So the SUT here must be a bare type like `E2ESut<Full>`.
    (
        test_fn: $test_fn:ident,
        $(#[$attr:meta])*
        inner_sut: $sut:ident $(< $($ty_param:tt),+ >)?,
        proptest_config: $config:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(,)?
    ) => {
        ::proptest_state_machine::prop_state_machine! {
            #![proptest_config($config)]
            $(#[$attr])*
            #[test]
            fn $test_fn(sequential $step_lo .. $step_hi => $sut $(< $($ty_param),+ >)?);
        }
    };

    // ── Slice mode, explicit `proptest_config:` ─────────────────────
    (
        test_fn: $test_fn:ident,
        variant_ref: $variant_ref:ty,
        inner_sut: $inner_sut:ty,
        transitions: [ $($transition_tts:tt)* ],
        invariants: [ $($invariant_tts:tt)* ],
        proptest_config: $config:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(, fixtures_dir: $fixtures_dir:literal)?
        $(,)?
    ) => {
        $crate::__declare_pbt_slice_wrapper!(
            $test_fn, $variant_ref, $inner_sut,
            [ $($transition_tts)* ], [ $($invariant_tts)* ],
            $config, $step_lo, $step_hi
            $(, $fixtures_dir)?
        );
    };

    // ── Slice mode, `cases:` + `max_shrink_iters:` ──────────────────
    (
        test_fn: $test_fn:ident,
        variant_ref: $variant_ref:ty,
        inner_sut: $inner_sut:ty,
        transitions: [ $($transition_tts:tt)* ],
        invariants: [ $($invariant_tts:tt)* ],
        cases: $cases:expr,
        max_shrink_iters: $max_shrink_iters:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(, fixtures_dir: $fixtures_dir:literal)?
        $(,)?
    ) => {
        $crate::__declare_pbt_slice_wrapper!(
            $test_fn, $variant_ref, $inner_sut,
            [ $($transition_tts)* ], [ $($invariant_tts)* ],
            ::proptest::test_runner::Config {
                cases: $cases,
                max_shrink_iters: $max_shrink_iters,
                ..::proptest::test_runner::Config::default()
            },
            $step_lo, $step_hi
            $(, $fixtures_dir)?
        );
    };
}

/// Internal: the wrapper-mode body shared by both slice arms. Takes a
/// fully-resolved `$config:expr` so the public arms only differ in how
/// they construct the `ProptestConfig`.
#[doc(hidden)]
#[macro_export]
macro_rules! __declare_pbt_slice_wrapper {
    (
        $test_fn:ident, $variant_ref:ty, $inner_sut:ty,
        [ $($transition_tts:tt)* ], [ $($invariant_tts:tt)* ],
        $config:expr, $step_lo:tt, $step_hi:tt
        $(, $fixtures_dir:literal)?
    ) => {
        ::paste::paste! {
            pub struct [<$test_fn:camel Machine>];

            impl ::proptest_state_machine::ReferenceStateMachine
                for [<$test_fn:camel Machine>]
            {
                type State = $variant_ref;
                type Transition = $crate::pbt::transitions::E2ETransition;

                fn init_state() -> ::proptest::strategy::BoxedStrategy<Self::State> {
                    <$variant_ref as ::proptest_state_machine::ReferenceStateMachine>::init_state()
                }

                fn transitions(state: &Self::State) -> ::proptest::strategy::BoxedStrategy<Self::Transition> {
                    use ::proptest::strategy::{Strategy, Union};
                    use $crate::pbt::transitions::E2ETransition;
                    let mut arms: Vec<(u32, ::proptest::strategy::BoxedStrategy<E2ETransition>)>
                        = Vec::new();
                    $crate::__build_transition_arms!(arms, &**state, $($transition_tts)*);
                    assert!(
                        !arms.is_empty(),
                        concat!(stringify!($test_fn), ": no transition applicable")
                    );
                    Union::new_weighted(arms).boxed()
                }

                fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
                    use ::holon_pbt_core::TransitionRef;
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
                    use ::holon_pbt_core::TransitionRef;
                    transition.apply_to_ref(&mut *state);
                    state.last_transition_kind = Some(transition.variant_name());
                    state
                }
            }

            pub struct [<$test_fn:camel Sut>] {
                inner: $inner_sut,
            }

            impl ::std::ops::Drop for [<$test_fn:camel Sut>] {
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

            impl ::proptest_state_machine::StateMachineTest for [<$test_fn:camel Sut>] {
                type SystemUnderTest = Self;
                type Reference = [<$test_fn:camel Machine>];

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
                    [<$test_fn:camel Sut>] {
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
                    let sut_inner = &sut.inner;
                    runtime.block_on(async {
                        $crate::__build_invariant_arms!(ref_inner, sut_inner, $($invariant_tts)*);
                    });
                }
            }

            // Bridge so the generic fixture runner can evaluate `Then`
            // assertions against this slice's SUT — delegates to the inner
            // `E2ESut`'s `FixtureAssertable` impl (which owns the capabilities).
            impl $crate::pbt::fixtures::FixtureAssertable for [<$test_fn:camel Sut>] {
                fn evaluate_assert(
                    &self,
                    assertion: &$crate::pbt::fixtures::assert::Assertion,
                    ref_state: &$crate::pbt::ReferenceState,
                ) -> ::std::result::Result<(), ::std::string::String> {
                    $crate::pbt::fixtures::FixtureAssertable::evaluate_assert(
                        &self.inner,
                        assertion,
                        ref_state,
                    )
                }
            }

            ::proptest_state_machine::prop_state_machine! {
                #![proptest_config($config)]
                #[test]
                fn $test_fn(sequential $step_lo .. $step_hi => [<$test_fn:camel Sut>]);
            }

            // Optional: replay every `*.json` (machine-captured) and
            // `*.feature` (hand-authored) fixture under `fixtures_dir` before
            // the proptest sweep gets a chance to find new regressions. Strict
            // semantics: a precondition that no longer holds is a hard panic,
            // so stale fixtures fail loud instead of silently passing.
            $(
                #[test]
                fn [<$test_fn _fixtures>]() {
                    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/", $fixtures_dir);
                    let sources: ::std::vec::Vec<
                        ::std::boxed::Box<dyn $crate::pbt::fixtures::FixtureSource>,
                    > = ::std::vec![
                        ::std::boxed::Box::new(
                            $crate::pbt::fixtures::json::JsonFixtureSource::new(dir),
                        ),
                        ::std::boxed::Box::new(
                            $crate::pbt::fixtures::gherkin::GherkinFixtureSource::new(dir),
                        ),
                    ];
                    $crate::pbt::fixtures::run_fixtures::<
                        [<$test_fn:camel Machine>],
                        [<$test_fn:camel Sut>],
                        _,
                    >(&sources);
                }
            )?
        }
    };
}

// ── Capture-on-panic and fixture-dir replay helpers ───────────────

use crate::pbt::transitions::E2ETransition;
use holon_pbt_core::fixture::Fixture;
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

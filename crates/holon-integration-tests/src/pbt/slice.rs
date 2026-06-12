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
                InvariantResult::Skipped(reason) => {
                    ::tracing::info!(
                        invariant = ::std::any::type_name_of_val(&$invariant),
                        %reason,
                        "slice invariant skipped"
                    );
                }
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
        // ADR 0007 item 3: manifest gate (necessary, not sufficient) + PCG-2 cap gate.
        if <$ty as ::holon_pbt_core::TransitionFactory<
            $crate::pbt::reference_state::ReferenceState,
        >>::required_wiring()
        .satisfied_by(&$state.wiring)
            && $state.caps_available(&<$ty as ::holon_pbt_core::TransitionFactory<
                $crate::pbt::reference_state::ReferenceState,
            >>::required_caps())
        {
            if let ::validated::Validated::Good((w, s)) =
                <$ty as ::holon_pbt_core::TransitionFactory<
                    $crate::pbt::reference_state::ReferenceState,
                >>::weighted_generator($state)
            {
                let filtered = s
                    .prop_filter($reason, $filter)
                    .prop_map($crate::pbt::transitions::E2ETransition::from)
                    .boxed();
                $arms.push((w, filtered));
            }
        }
    }};
    // Plain: bare type path, no filter. Routes through the shared
    // `holon_pbt_core::weighted_arm` helper (same aggregation path as
    // `aggregate_transitions`); slice generators discard rejection
    // reasons — `preconditions` records them on its own pass.
    ($arms:ident, $state:expr, $ty:path) => {{
        // ADR 0007 item 3: manifest gate (necessary, not sufficient) + PCG-2 cap gate.
        if <$ty as ::holon_pbt_core::TransitionFactory<
            $crate::pbt::reference_state::ReferenceState,
        >>::required_wiring()
        .satisfied_by(&$state.wiring)
            && $state.caps_available(&<$ty as ::holon_pbt_core::TransitionFactory<
                $crate::pbt::reference_state::ReferenceState,
            >>::required_caps())
        {
            if let ::validated::Validated::Good(Some(arm)) =
                ::holon_pbt_core::weighted_arm::<_, $ty, $crate::pbt::transitions::E2ETransition>(
                    $state,
                    1,
                    |v| $crate::pbt::transitions::E2ETransition::from(v),
                )
            {
                $arms.push(arm);
            }
        }
    }};
}

/// Declare a PBT slice with the minimum boilerplate.
///
/// ```ignore
/// declare_pbt_slice! {
///     test_fn: my_slice_pbt,
///     wiring: ::holon_pbt_core::Wiring::sql_only(),
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
/// Omit `transitions:` and `invariants:` to run the **full-coverage** mode:
/// the generated machine drives the full transition alphabet
/// (`aggregate_transitions`) and the SUT runs the full invariant registry
/// (`run_invariant_registry`), both seeded from `wiring:`. This is the right
/// mode for broad end-to-end sweeps (cf. `general_e2e_pbt`).
/// `proptest_config:` is required:
///
/// ```ignore
/// declare_pbt_slice! {
///     test_fn: general_e2e_pbt,
///     wiring: ::holon_pbt_core::Wiring::full(),
///     proptest_config: pbt_config(),
///     steps: 3..20,
/// }
/// ```
#[macro_export]
macro_rules! declare_pbt_slice {
    // ── Native (full-coverage) mode ─────────────────────────────────
    // Generates a per-manifest wrapper machine + SUT that runs the full
    // transition alphabet (`aggregate_transitions`) and the full invariant
    // registry (`run_invariant_registry`), seeded from the slice's `wiring`.
    (
        test_fn: $test_fn:ident,
        wiring: $wiring:expr,
        proptest_config: $config:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(,)?
    ) => {
        $crate::__declare_pbt_full_slice!(
            $test_fn,
            ::proptest::strategy::Strategy::boxed(::proptest::strategy::Just(
                $crate::pbt::fresh_reference_state($wiring),
            )),
            false,
            $config, $step_lo, $step_hi
        );
    };

    // ── Slice mode, explicit `proptest_config:` ─────────────────────
    (
        test_fn: $test_fn:ident,
        wiring: $wiring:expr,
        transitions: [ $($transition_tts:tt)* ],
        invariants: [ $($invariant_tts:tt)* ],
        proptest_config: $config:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(, fixtures_dir: $fixtures_dir:literal)?
        $(,)?
    ) => {
        $crate::__declare_pbt_slice_wrapper!(
            $test_fn, $wiring,
            [ $($transition_tts)* ], [ $($invariant_tts)* ],
            $config, $step_lo, $step_hi
            $(, $fixtures_dir)?
        );
    };

    // ── Slice mode, `cases:` + `max_shrink_iters:` ──────────────────
    (
        test_fn: $test_fn:ident,
        wiring: $wiring:expr,
        transitions: [ $($transition_tts:tt)* ],
        invariants: [ $($invariant_tts:tt)* ],
        cases: $cases:expr,
        max_shrink_iters: $max_shrink_iters:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(, fixtures_dir: $fixtures_dir:literal)?
        $(,)?
    ) => {
        $crate::__declare_pbt_slice_wrapper!(
            $test_fn, $wiring,
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

/// The **faithful subsystem-convergence harness** (F2 / Plan 2). A thin arm over
/// the same [`__declare_pbt_full_slice!`] expansion the blessed slices use — so
/// it shares their capture/replay parity exactly — but it replaces the constant
/// `init_state` (`Just(fresh_reference_state($wiring))`, immune to shrinking)
/// with a **generated, shrinkable** [`Wiring`](holon_pbt_core::any_valid_wiring):
/// proptest draws which subsystems are active, drives the real booted `E2ESut`
/// through the full production alphabet, runs the wiring-gated invariant
/// registry, and on failure **shrinks the active subsystem set toward the
/// minimal combination that still reproduces**.
///
/// Unlike a blessed slice, there is no `wiring:` — the wiring *is* the generated
/// input. `init_test` reads `ref_state.wiring` at runtime to select the backend
/// (`storage_selector_for_wiring`), so a Loro-only draw boots the cheap
/// `LoroMemory` SUT and a Turso draw boots the full `BackendEngine`. The `$boot_guard`
/// (always `true` here) makes `teardown` fail loud if a non-trivial case never
/// fired `StartApp` — guarding against vacuous pre-startup-only green.
///
/// This harness is **not `#[ignore]`d** — it runs under a default `cargo test`.
/// Because it shares the blessed slices' expansion and the default `wiring_axes`
/// generate over `{Loro, Org, Turso}` (Turso down-weighted), it **subsumes** the
/// per-Wiring blessed slices — each pins one point in the generated powerset. The
/// first retired this way was `loro_backend_pbt` (pinned `{Loro}`), now one draw of
/// this harness. Scope/​widen a run with `HOLON_PBT_WIRING_AXES`.
///
/// ```ignore
/// declare_pbt_convergence! {
///     test_fn: subsystem_convergence_pbt,
///     proptest_config: ::proptest::test_runner::Config {
///         cases: 4, max_shrink_iters: 8, ..Default::default()
///     },
///     steps: 3..12,
/// }
/// ```
#[macro_export]
macro_rules! declare_pbt_convergence {
    (
        test_fn: $test_fn:ident,
        proptest_config: $config:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(,)?
    ) => {
        $crate::__declare_pbt_full_slice!(
            $test_fn,
            ::proptest::strategy::Strategy::boxed(::proptest::strategy::Strategy::prop_map(
                ::holon_pbt_core::any_valid_wiring(),
                $crate::pbt::fresh_reference_state,
            )),
            true,
            $config,
            $step_lo,
            $step_hi // No `test_attrs` ⇒ the harness runs under a default `cargo test`
                     // (no longer `#[ignore]`d). It is fast enough with the default
                     // `wiring_axes` (Turso down-weighted, most draws boot the cheap
                     // `LoroMemory` SUT); scope a run further with `HOLON_PBT_WIRING_AXES`.
        );
    };
}

/// One-line, `ComponentSet`-driven slice (ADR 0009 Goal 1). A thin sugar over
/// [`declare_pbt_slice!`]: it lowers a [`ComponentSet`](holon_pbt_core::ComponentSet)
/// to its `wiring` and delegates. The point is that a slice is named by *what
/// components it composes*, so widening/narrowing is a one-token edit to the set.
///
/// ```ignore
/// use holon_pbt_core::ComponentSet;
/// use holon_pbt_core::component_set::Component::*;
/// use holon_pbt_core::StorageAdapter::Loro;
///
/// component_pbt! {
///     test_fn: loro_vm_fast_pbt,
///     set: ComponentSet::loro_vm_fast(),       // or `ComponentSet::of([Loro.into(), ViewModel.into()])`
///     proptest_config: pbt_config(),
///     steps: 3..20,
/// }
/// ```
///
/// The **projection axis** of the set (`ViewModel`/`EditorState`) is a
/// *bisection* concept — the invariant runner always observes both at runtime
/// (`run_invariant_registry` builds the live set from `wiring` + both
/// projections). A single slice's faithful knob is therefore the wiring, which
/// the set carries; dropping a projection only changes which lattice node a
/// bisection oracle builds, not which invariants a standalone slice runs. So
/// lowering `set` → `set.wiring` loses nothing a slice could express.
#[macro_export]
macro_rules! component_pbt {
    // ── Native (full-coverage) mode ─────────────────────────────────
    (
        test_fn: $test_fn:ident,
        set: $set:expr,
        proptest_config: $config:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(,)?
    ) => {
        $crate::declare_pbt_slice! {
            test_fn: $test_fn,
            wiring: ($set).wiring,
            proptest_config: $config,
            steps: $step_lo .. $step_hi
        }
    };

    // ── Slice mode (explicit transitions/invariants), cases-based ───
    (
        test_fn: $test_fn:ident,
        set: $set:expr,
        transitions: [ $($transition_tts:tt)* ],
        invariants: [ $($invariant_tts:tt)* ],
        cases: $cases:expr,
        max_shrink_iters: $max_shrink_iters:expr,
        steps: $step_lo:tt .. $step_hi:tt
        $(, fixtures_dir: $fixtures_dir:literal)?
        $(,)?
    ) => {
        $crate::declare_pbt_slice! {
            test_fn: $test_fn,
            wiring: ($set).wiring,
            transitions: [ $($transition_tts)* ],
            invariants: [ $($invariant_tts)* ],
            cases: $cases,
            max_shrink_iters: $max_shrink_iters,
            steps: $step_lo .. $step_hi
            $(, fixtures_dir: $fixtures_dir)?
        }
    };
}

/// Internal: the wrapper-mode body shared by both slice arms. Takes a
/// fully-resolved `$config:expr` so the public arms only differ in how
/// they construct the `ProptestConfig`.
#[doc(hidden)]
#[macro_export]
macro_rules! __declare_pbt_slice_wrapper {
    (
        $test_fn:ident, $wiring:expr,
        [ $($transition_tts:tt)* ], [ $($invariant_tts:tt)* ],
        $config:expr, $step_lo:tt, $step_hi:tt
        $(, $fixtures_dir:literal)?
    ) => {
        ::paste::paste! {
            pub struct [<$test_fn:camel Machine>];

            impl ::proptest_state_machine::ReferenceStateMachine
                for [<$test_fn:camel Machine>]
            {
                type State = $crate::pbt::ReferenceState;
                type Transition = $crate::pbt::transitions::E2ETransition;

                fn init_state() -> ::proptest::strategy::BoxedStrategy<Self::State> {
                    ::proptest::strategy::Strategy::boxed(::proptest::strategy::Just(
                        $crate::pbt::fresh_reference_state($wiring),
                    ))
                }

                fn transitions(state: &Self::State) -> ::proptest::strategy::BoxedStrategy<Self::Transition> {
                    use ::proptest::strategy::{Strategy, Union};
                    use $crate::pbt::transitions::E2ETransition;
                    let mut arms: Vec<(u32, ::proptest::strategy::BoxedStrategy<E2ETransition>)>
                        = Vec::new();
                    $crate::__build_transition_arms!(arms, state, $($transition_tts)*);
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
                    // Value-level wiring gate, mirroring `aggregate_transitions`'
                    // alphabet gate (`transition_dispatch.rs`). Generation only
                    // ever offers wiring-satisfied transitions, so this is a no-op
                    // for the fixed-wiring slices — but the convergence harness
                    // shrinks the *generated manifest itself*. When a wiring-shrink
                    // would strand a transition the smaller manifest no longer
                    // structurally supports, this makes that transition fail its
                    // precondition; `proptest-state-machine`'s `check_acceptable`
                    // then *rejects that shrink* rather than replaying an
                    // inapplicable transition on the wrong backend. (Make-or-break
                    // #1c: wiring-dropped transitions are skipped, never panicked.)
                    if !transition.required_wiring().satisfied_by(&state.wiring)
                        || !state.caps_available(&transition.required_caps())
                    {
                        return false;
                    }
                    match transition.preconditions(state) {
                        Validated::Good(()) => true,
                        Validated::Fail(reasons) => {
                            record_rejection(transition.variant_name(), &reasons);
                            false
                        }
                    }
                }

                fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
                    use ::holon_pbt_core::TransitionRef;
                    transition.apply_to_ref(&mut state);
                    state.action.last_transition_kind = Some(transition.variant_name());
                    state
                }
            }

            pub struct [<$test_fn:camel Sut>] {
                inner: $crate::pbt::E2ESut,
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
                    ref_state: &<Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::State,
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
                    $crate::pbt::slice::record_capture_wiring(&ref_state.wiring);
                    // The slice's `wiring:` selects the SUT backend (ADR 0004
                    // Phase 9, part (a)): a Loro-only manifest builds the no-Turso
                    // `LoroMemory` SUT, anything with Turso builds the Turso SUT.
                    let storage = $crate::pbt::storage_selector_for_wiring(&ref_state.wiring);
                    [<$test_fn:camel Sut>] {
                        inner: $crate::pbt::E2ESut::new_with_backend(runtime, storage).unwrap(),
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
                    let ref_inner: &$crate::pbt::ReferenceState = ref_state;
                    let sut_inner = &sut.inner;
                    runtime.block_on(async {
                        $crate::__build_invariant_arms!(ref_inner, sut_inner, $($invariant_tts)*);
                    });
                }

                // Route the per-case loop through the shared engine
                // (`stepper::run_sequence`) instead of the library default, so
                // the headless and GPUI runners share one loop body (ADR 0009
                // §5). Generation/shrinking/regression replay are unchanged —
                // they live in `sequential_strategy`, above this call.
                fn test_sequential(
                    _config: ::proptest::test_runner::Config,
                    ref_state: $crate::pbt::ReferenceState,
                    transitions: ::std::vec::Vec<$crate::pbt::transitions::E2ETransition>,
                    seen_counter: ::std::option::Option<
                        ::std::sync::Arc<::std::sync::atomic::AtomicUsize>,
                    >,
                ) {
                    $crate::pbt::stepper::run_via_state_machine_test::<Self>(
                        ref_state, transitions, seen_counter,
                    );
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
                    >(&sources);
                }
            )?
        }
    };
}

/// Internal: full-coverage (native) slice. Generates a per-manifest machine
/// (`aggregate_transitions` alphabet, seeded from `$wiring`) and a SUT wrapper
/// that runs the full invariant registry (`run_invariant_registry`) — the
/// behavior of the old bare-`E2ESut` native mode, now wiring-parameterized.
#[doc(hidden)]
#[macro_export]
macro_rules! __declare_pbt_full_slice {
    (
        $test_fn:ident, $init:expr, $boot_guard:expr,
        $config:expr, $step_lo:tt, $step_hi:tt
        $(, test_attrs: [ $(#[$test_meta:meta])* ])?
    ) => {
        ::paste::paste! {
            pub struct [<$test_fn:camel Machine>];

            impl ::proptest_state_machine::ReferenceStateMachine
                for [<$test_fn:camel Machine>]
            {
                type State = $crate::pbt::ReferenceState;
                type Transition = $crate::pbt::transitions::E2ETransition;

                fn init_state() -> ::proptest::strategy::BoxedStrategy<Self::State> {
                    $init
                }

                fn transitions(state: &Self::State) -> ::proptest::strategy::BoxedStrategy<Self::Transition> {
                    $crate::pbt::transitions::aggregate_transitions(state)
                }

                fn preconditions(state: &Self::State, transition: &Self::Transition) -> bool {
                    use ::holon_pbt_core::TransitionRef;
                    use $crate::pbt::validation::record_rejection;
                    use ::validated::Validated;
                    // Value-level wiring gate, mirroring `aggregate_transitions`'
                    // alphabet gate (`transition_dispatch.rs`). Generation only
                    // ever offers wiring-satisfied transitions, so this is a no-op
                    // for the fixed-wiring slices — but the convergence harness
                    // shrinks the *generated manifest itself*. When a wiring-shrink
                    // would strand a transition the smaller manifest no longer
                    // structurally supports, this makes that transition fail its
                    // precondition; `proptest-state-machine`'s `check_acceptable`
                    // then *rejects that shrink* rather than replaying an
                    // inapplicable transition on the wrong backend. (Make-or-break
                    // #1c: wiring-dropped transitions are skipped, never panicked.)
                    if !transition.required_wiring().satisfied_by(&state.wiring)
                        || !state.caps_available(&transition.required_caps())
                    {
                        return false;
                    }
                    match transition.preconditions(state) {
                        Validated::Good(()) => true,
                        Validated::Fail(reasons) => {
                            record_rejection(transition.variant_name(), &reasons);
                            false
                        }
                    }
                }

                fn apply(mut state: Self::State, transition: &Self::Transition) -> Self::State {
                    use ::holon_pbt_core::TransitionRef;
                    transition.apply_to_ref(&mut state);
                    state.action.last_transition_kind = Some(transition.variant_name());
                    state
                }
            }

            pub struct [<$test_fn:camel Sut>] {
                inner: $crate::pbt::E2ESut,
            }

            impl ::std::ops::Drop for [<$test_fn:camel Sut>] {
                fn drop(&mut self) {
                    if ::std::thread::panicking() {
                        $crate::pbt::slice::write_captured_fixture(stringify!($test_fn));
                    }
                }
            }

            impl ::proptest_state_machine::StateMachineTest for [<$test_fn:camel Sut>] {
                type SystemUnderTest = Self;
                type Reference = [<$test_fn:camel Machine>];

                fn init_test(
                    ref_state: &<Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::State,
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
                    $crate::pbt::slice::record_capture_wiring(&ref_state.wiring);
                    // The slice's `wiring:` selects the SUT backend (ADR 0004
                    // Phase 9, part (a)): a Loro-only manifest builds the no-Turso
                    // `LoroMemory` SUT, anything with Turso builds the Turso SUT.
                    let storage = $crate::pbt::storage_selector_for_wiring(&ref_state.wiring);
                    [<$test_fn:camel Sut>] {
                        inner: $crate::pbt::E2ESut::new_with_backend(runtime, storage).unwrap(),
                    }
                }

                fn apply(
                    mut sut: Self::SystemUnderTest,
                    ref_state:
                        &<Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::State,
                    transition:
                        <Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::Transition,
                ) -> Self::SystemUnderTest {
                    $crate::pbt::slice::record_transition(&transition);
                    sut.inner.drive_transition(ref_state, &transition);
                    sut
                }

                fn check_invariants(
                    sut: &Self::SystemUnderTest,
                    ref_state:
                        &<Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::State,
                ) {
                    let runtime = sut.inner.runtime.clone();
                    runtime.block_on(sut.inner.run_invariant_registry(ref_state));
                }

                // End-of-case sweep: run the `NotNavOnly`-gated invariants
                // once against the final state, so a nav-tail sequence
                // doesn't end unchecked (mirrors the native E2E impl).
                fn teardown(
                    sut: Self::SystemUnderTest,
                    ref_state:
                        <Self::Reference as ::proptest_state_machine::ReferenceStateMachine>::State,
                ) {
                    let runtime = sut.inner.runtime.clone();
                    runtime.block_on(sut.inner.run_invariant_registry_end_of_case(&ref_state));
                    // Visible "did it actually boot" guard (convergence harness only —
                    // `$boot_guard` folds to `false` for the blessed slices). A
                    // non-trivial case (>= `$step_lo` applied transitions) that never
                    // fired `StartApp` exercised nothing on a booted app — the vacuous
                    // pre-startup-only coverage the plan flags. It is surfaced *loudly
                    // but non-fatally* (project error philosophy: "falls back visibly —
                    // clearly signals degraded mode"): a panic here is wrong because
                    // (a) a randomly-drawn pre-startup-only sequence is not a bug, and
                    // (b) a teardown panic corrupts `proptest-state-machine`'s
                    // `seen_transitions_counter` shrink bookkeeping. The nightly job
                    // greps these lines to confirm coverage is real.
                    if $boot_guard {
                        let applied = $crate::pbt::slice::captured_transition_count();
                        if !ref_state.action.app_started && applied >= $step_lo {
                            ::tracing::warn!(
                                target: "pbt_convergence",
                                applied,
                                "VACUOUS CASE: applied {applied} transition(s) but StartApp \
                                 never fired (app_started=false) — exercised nothing on a \
                                 booted app",
                            );
                            ::std::eprintln!(
                                "[convergence] VACUOUS CASE: applied {applied} transition(s) \
                                 but StartApp never fired — exercised nothing on a booted app",
                            );
                        }
                    }
                }

                // Shared-engine loop (ADR 0009 §5); see the selective-slice
                // macro for rationale.
                fn test_sequential(
                    _config: ::proptest::test_runner::Config,
                    ref_state: $crate::pbt::ReferenceState,
                    transitions: ::std::vec::Vec<$crate::pbt::transitions::E2ETransition>,
                    seen_counter: ::std::option::Option<
                        ::std::sync::Arc<::std::sync::atomic::AtomicUsize>,
                    >,
                ) {
                    $crate::pbt::stepper::run_via_state_machine_test::<Self>(
                        ref_state, transitions, seen_counter,
                    );
                }
            }

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
                $($(#[$test_meta])*)?
                fn $test_fn(sequential $step_lo .. $step_hi => [<$test_fn:camel Sut>]);
            }
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
    wiring: Option<holon_pbt_core::Wiring>,
    already_written: bool,
}

pub fn reset_capture(_: &'static str) {
    CAPTURE.with(|c| {
        *c.borrow_mut() = Some(CaptureState {
            transitions: Vec::new(),
            wiring: None,
            already_written: false,
        });
    });
}

/// Record the wiring the capture is being generated under (called once the
/// run's `Wiring` is known — it may not be at `reset_capture` time).
pub fn record_capture_wiring(wiring: &holon_pbt_core::Wiring) {
    CAPTURE.with(|c| {
        if let Some(state) = c.borrow_mut().as_mut() {
            state.wiring = Some(wiring.clone());
        }
    });
}

pub fn record_transition(t: &E2ETransition) {
    CAPTURE.with(|c| {
        if let Some(state) = c.borrow_mut().as_mut() {
            state.transitions.push(t.clone());
        }
    });
}

/// Number of transitions applied (recorded) so far in the active case. The
/// convergence harness uses this in `teardown` to tell a non-trivial case from
/// a shrunk near-empty one when checking that `StartApp` actually fired.
pub fn captured_transition_count() -> usize {
    CAPTURE.with(|c| {
        c.borrow()
            .as_ref()
            .map(|state| state.transitions.len())
            .unwrap_or(0)
    })
}

/// Write the captured transition sequence to
/// `$CARGO_MANIFEST_DIR/tests/.captures/<slice>.captured.json`. Idempotent
/// per panic (proptest may unwind through multiple Drop sites during
/// shrink — first writer wins).
pub fn write_captured_fixture(slice_name: &str) {
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
            environment: holon_pbt_core::fixture::CaptureEnvironment {
                wiring: state.wiring.clone(),
                env_flags: holon_pbt_core::fixture::CaptureEnvironment::current_env_flags(),
            },
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

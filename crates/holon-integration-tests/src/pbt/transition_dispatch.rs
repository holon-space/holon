//! File-per-transition pattern for the E2E PBT.
//!
//! Each transition kind owns its data (a struct), generation strategy
//! (`holon_pbt_core::TransitionFactory<ReferenceState>`), and behaviour
//! (`holon_pbt_core::TransitionImpl<ReferenceState, dyn SutHandle>`). The
//! `declare_e2e_transitions!` macro generates the dispatch enum, the
//! `From<variant>` impls, the hand-rolled trait dispatch on the enum, the
//! `SqlBudget` dispatch, and the strategy aggregator.
//!
//! See `experiments/enum-dispatch-examples/pbt-state-machine/` for the
//! standalone proof-of-concept; this module ports that pattern into
//! the real PBT.
//!
//! ## Cross-PBT trait vocabulary
//!
//! Transitions implement the generic `holon_pbt_core::{TransitionFactory,
//! TransitionImpl}` traits (shared with the layout / editor-pure PBTs)
//! rather than a PBT-local trait. The enum dispatches them by a hand-rolled
//! `match` (a foreign generic trait can't be `enum_dispatch`ed). The
//! integration-test-only SQL budget is split into a separate
//! `transition_budgets::SqlBudget` trait so the shared behaviour trait stays
//! medium-agnostic.
//!
//! - `SutHandle` (this file) is the object-safe view a transition
//!   uses to drive the SUT. `#[async_trait]` so `&mut dyn SutHandle`
//!   works under generic-V SUTs. The trait grows methods as variants
//!   are migrated; today it is empty.

/// Per-variant weight multiplier read from the `HOLON_PBT_WEIGHTS`
/// environment variable. The macro `declare_e2e_transitions!` applies
/// this automatically to every arm of `aggregate_transitions`, so
/// individual variants don't need to wire it themselves.
///
/// Format: comma-separated `pattern:multiplier` pairs.
/// `pattern` is matched case-insensitively against the variant name.
/// Patterns may contain a single `*` glob for prefix / suffix /
/// contains matching.
///
/// Examples:
///
/// ```text
/// HOLON_PBT_WEIGHTS=Indent:200            # boost a single variant
/// HOLON_PBT_WEIGHTS=Indent:200,Outdent:200,ToggleState:200,Move*:100
/// HOLON_PBT_WEIGHTS=*Edit*:0,Click*:50    # silence one family, boost another
/// ```
///
/// Multiplier `0` removes the variant from the strategy entirely
/// (`weighted_generator` still computes its base weight, but
/// `Union::new_weighted` drops zero-weight arms). Multiplier defaults
/// to `1` for any variant not matched by any pattern.
///
/// Why a single env var: previously each "interesting family"
/// (chord ops, navigation, edit) needed its own bespoke env var
/// wired into each variant's `weighted_generator`. The macro-applied
/// generic shim lets future verification work tune any subset
/// without touching transition source.
pub fn variant_weight_multiplier(variant_name: &'static str) -> u32 {
    use std::sync::OnceLock;

    static PARSED: OnceLock<Vec<(WeightPattern, u32)>> = OnceLock::new();
    let rules = PARSED.get_or_init(parse_weight_env);
    if rules.is_empty() {
        return 1;
    }
    for (pattern, mult) in rules {
        if pattern.matches(variant_name) {
            return *mult;
        }
    }
    1
}

#[derive(Debug)]
enum WeightPattern {
    Exact(String),
    Prefix(String),
    Suffix(String),
    Contains(String),
    Star, // bare `*` → matches every variant
}

impl WeightPattern {
    fn matches(&self, name: &str) -> bool {
        let n = name.to_ascii_lowercase();
        match self {
            Self::Exact(s) => n == *s,
            Self::Prefix(s) => n.starts_with(s),
            Self::Suffix(s) => n.ends_with(s),
            Self::Contains(s) => n.contains(s),
            Self::Star => true,
        }
    }
}

fn parse_weight_env() -> Vec<(WeightPattern, u32)> {
    let raw = match std::env::var("HOLON_PBT_WEIGHTS") {
        Ok(s) if !s.is_empty() => s,
        _ => return Vec::new(),
    };
    let mut rules = Vec::new();
    for entry in raw.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let Some((pat_raw, mult_raw)) = entry.split_once(':') else {
            eprintln!("[HOLON_PBT_WEIGHTS] ignoring '{entry}': expected `pattern:multiplier`");
            continue;
        };
        let mult: u32 = match mult_raw.trim().parse() {
            Ok(n) => n,
            Err(_) => {
                eprintln!("[HOLON_PBT_WEIGHTS] ignoring '{entry}': multiplier must be a u32");
                continue;
            }
        };
        let pat_lower = pat_raw.trim().to_ascii_lowercase();
        let pattern = if pat_lower == "*" {
            WeightPattern::Star
        } else if let Some(stripped) = pat_lower.strip_prefix('*') {
            if let Some(middle) = stripped.strip_suffix('*') {
                WeightPattern::Contains(middle.to_string())
            } else {
                WeightPattern::Suffix(stripped.to_string())
            }
        } else if let Some(stripped) = pat_lower.strip_suffix('*') {
            WeightPattern::Prefix(stripped.to_string())
        } else {
            WeightPattern::Exact(pat_lower)
        };
        rules.push((pattern, mult));
    }
    rules
}

// TODO- Split this up into smaller Sut structs. Some of these might already exist
/// Coarse SUT capability bundle for the E2E PBT. Transitions are
/// dispatched generically over `S: SutHandle` (concrete-`S` dispatch,
/// no `dyn`), so the trait uses **native `async fn`** rather than
/// `#[async_trait]` boxing. This is what lets a transition's
/// `TransitionImpl<R, S>` impl be narrowed to fine-grained capability
/// bounds (`S: SutEditorMirrorWrite`, …) while the enum still dispatches
/// `S: SutHandle` — `SutHandle` is (becoming) a supertrait bundle of the
/// fine caps.
///
/// Non-`Send` futures: `E2ESut<V>` holds `RefCell` fields (interior
/// mutability for the reactive engine) and can't be `Sync`. Native
/// `async fn` futures are `?Send` by default, which matches — proptest's
/// state-machine driver runs single-threaded via `runtime.block_on`.
///
/// Implemented for `E2ESut<V>` in `sut_handle.rs`. The trait grows
/// methods as variants are migrated; each migration that needs a new SUT
/// capability adds one method here and one impl in `sut_handle.rs`
/// (delegating to existing helpers).
/// `SutHandle` is now a pure **marker bundle**: it declares no methods of its
/// own — the entire transition alphabet is hosted by the fine-grained capability
/// traits below. The cluster-peel relocated every former `SutHandle::apply_*`
/// method into a `#[capmap_adapter]` cap (placed per the cap home-rule:
/// holon-api-typed → `holon-pbt-core`; frontend-typed → `holon-frontend`;
/// test-only-typed → `crate::pbt::local_caps`). `ref_state` is never in a cap
/// signature — action-time needs are precomputed at the transition boundary
/// (e.g. `StartApp`'s `root_id`) and settle/reconcile work lives in
/// `E2ESut::block_tree_post_action`.
///
/// Because the bundle is a pure conjunction of caps, a composed `CapMap` that
/// holds all of them satisfies `SutHandle` exactly as `E2ESut` does — which is
/// the whole point of the decomposition (E3/E5 can then delete `E2ESut`).
pub trait SutHandle:
    ::holon_pbt_core::capabilities::SutEditorMirrorWrite
    + ::holon_pbt_core::capabilities::SutBlockTreeWrite
    + ::holon_pbt_core::capabilities::SutEdgeFieldWrite
    + ::holon_pbt_core::capabilities::SutLoro
    + ::holon_pbt_core::capabilities::SutFocusWrite
    + ::holon_pbt_core::capabilities::SutNavHistoryWrite
    + ::holon_pbt_core::capabilities::SutWatchRegister
    + ::holon_pbt_core::capabilities::SutViewControl
    + ::holon_pbt_core::capabilities::SutMcpEmit
    + ::holon_pbt_core::capabilities::SutHistoryWrite
    + ::holon_pbt_core::capabilities::SutNavHistoryDrive
    + ::holon_pbt_core::capabilities::SutBlockInteract
    + ::holon_frontend::pbt_caps::SutArrowNavigate
    + crate::pbt::local_caps::SutMutate
    + crate::pbt::local_caps::SutSeamMutate
    + crate::pbt::transitions::apply_mutation::SutApplyMutation
    + crate::pbt::local_caps::SutFixtureFs
    + crate::pbt::local_caps::SutAppLifecycle
{
}

/// Blanket marker impl: any type providing the full capability bundle IS a
/// `SutHandle`. No explicit `impl SutHandle for E2ESut` is needed (or allowed
/// alongside this) — `E2ESut` implements every cap, so it satisfies the bundle
/// automatically, as will the composed `CapMap`.
impl<T> SutHandle for T where
    T: ::holon_pbt_core::capabilities::SutEditorMirrorWrite
        + ::holon_pbt_core::capabilities::SutBlockTreeWrite
        + ::holon_pbt_core::capabilities::SutEdgeFieldWrite
        + ::holon_pbt_core::capabilities::SutLoro
        + ::holon_pbt_core::capabilities::SutFocusWrite
        + ::holon_pbt_core::capabilities::SutNavHistoryWrite
        + ::holon_pbt_core::capabilities::SutWatchRegister
        + ::holon_pbt_core::capabilities::SutViewControl
        + ::holon_pbt_core::capabilities::SutMcpEmit
        + ::holon_pbt_core::capabilities::SutHistoryWrite
        + ::holon_pbt_core::capabilities::SutNavHistoryDrive
        + ::holon_pbt_core::capabilities::SutBlockInteract
        + ::holon_frontend::pbt_caps::SutArrowNavigate
        + crate::pbt::local_caps::SutMutate
        + crate::pbt::local_caps::SutSeamMutate
        + crate::pbt::transitions::apply_mutation::SutApplyMutation
        + crate::pbt::local_caps::SutFixtureFs
        + crate::pbt::local_caps::SutAppLifecycle
{
}

/// `cap_transition!` — terse, single-sourced SUT dispatch + cap declaration.
///
/// Generates a transition's `TransitionImpl<ReferenceState, S>` block (bound on
/// exactly one fine-grained cap) AND the matching `declared_caps()` from the
/// **same** cap token, so the dispatch bound and `TransitionFactory::required_caps`
/// can no longer drift. Any transition authored through this macro no longer needs
/// an entry in the `required_caps_match_transition_impl_bounds` guard test.
///
/// It is also the migration seam (see `docs/Testing/PbtCompositionDesign.md` §8.9):
/// the per-transition call site is agnostic to whether `apply_to_sut` dispatches
/// over a generic `S: Cap` (today) or a concrete composed `CapMap` (post-E5) —
/// flipping that is a change to *this macro's expansion*, not to any transition
/// file. The body calls `sut.<cap-method>(…)`, which works identically whether
/// `sut: &mut S` (S: Cap) or `sut: &mut CapMap` (CapMap implements every cap via
/// `#[capmap_adapter]`).
///
/// Single-cap form (the `TransitionFactory::required_caps` body becomes
/// `Self::declared_caps()`):
/// ```ignore
/// cap_transition! {
///     SplitBlock: SutBlockTreeWrite,
///     |me, _state, sut| { sut.apply_split_block(&me.block_id, me.position).await; }
/// }
/// ```
///
/// No-cap form (bound on the full `SutHandle` bundle; `required_caps` stays the
/// trait default of empty):
/// ```ignore
/// cap_transition! { Nothing, |_me, _state, _sut| {} }
/// ```
#[macro_export]
macro_rules! cap_transition {
    // ── single cap ──────────────────────────────────────────────────
    (
        $name:ident : $cap:path,
        | $me:ident, $state:ident, $sut:ident | $body:block
    ) => {
        impl $name {
            /// Single source of this transition's cap: `required_caps()` forwards
            /// here, and the `TransitionImpl` bound below binds the same `$cap`.
            pub(crate) fn declared_caps() -> ::std::vec::Vec<::holon_pbt_core::composition::CapId> {
                ::std::vec![::holon_pbt_core::composition::CapId::of::<dyn $cap>()]
            }
        }

        #[allow(async_fn_in_trait)]
        impl<S: $cap>
            ::holon_pbt_core::TransitionImpl<$crate::pbt::reference_state::ReferenceState, S>
            for $name
        {
            async fn apply_to_sut(
                &self,
                $state: &$crate::pbt::reference_state::ReferenceState,
                $sut: &mut S,
            ) {
                let $me = self;
                $body
            }
        }
    };

    // ── no cap (bound on the full SutHandle bundle) ──────────────────
    (
        $name:ident,
        | $me:ident, $state:ident, $sut:ident | $body:block
    ) => {
        #[allow(async_fn_in_trait)]
        impl<S: $crate::pbt::transition_dispatch::SutHandle>
            ::holon_pbt_core::TransitionImpl<$crate::pbt::reference_state::ReferenceState, S>
            for $name
        {
            async fn apply_to_sut(
                &self,
                $state: &$crate::pbt::reference_state::ReferenceState,
                $sut: &mut S,
            ) {
                let $me = self;
                $body
            }
        }
    };
}

/// `declare_e2e_transitions!` — the only central code in the file-
/// per-transition pattern.
///
/// Wraps `declarative_enum_dispatch::enum_dispatch!` (which generates
/// the trait, the enum, and the trait-for-enum dispatch impl using
/// only `macro_rules!` — no proc macros) and adds the proptest-state-
/// machine strategy aggregator.
///
/// Adding a transition = create the file + add one line to the call.
#[macro_export]
macro_rules! declare_e2e_transitions {
    (
        $vis:vis enum $enum_name:ident {
            $($variant:ident($ty:ty)),* $(,)?
        }
    ) => {
        #[derive(Clone, Debug, ::serde::Serialize, ::serde::Deserialize)]
        $vis enum $enum_name {
            $( $variant($ty) ),*
        }

        $(
            impl ::core::convert::From<$ty> for $enum_name {
                fn from(v: $ty) -> Self { $enum_name::$variant(v) }
            }
        )*

        // Dispatch the FOREIGN generic behaviour trait onto the enum by a
        // hand-rolled match (replaces `declarative_enum_dispatch`, which can
        // only define+dispatch its own local trait, not a foreign generic
        // one). Concrete-`S` dispatch: generic over `S: SutHandle` rather
        // than `dyn SutHandle`, so the SUT is monomorphised (the runner
        // passes `&mut E2ESut<V>`). This is what lets each variant's
        // `TransitionImpl<R, S>` impl narrow `S` to fine-grained capability
        // bounds — `SutHandle` is a supertrait bundle of those caps, so a
        // variant bound on `S: SutEditorMirrorWrite` still satisfies this
        // enum impl's `S: SutHandle`.
        // Ref-side dispatch (S-independent): preconditions + apply_to_ref.
        impl ::holon_pbt_core::TransitionRef<$crate::pbt::reference_state::ReferenceState>
            for $enum_name {
            type Reason = $crate::pbt::validation::Reason;

            fn preconditions(
                &self,
                state: &$crate::pbt::reference_state::ReferenceState,
            ) -> ::validated::Validated<(), $crate::pbt::validation::Reason> {
                match self {
                    $( $enum_name::$variant(v) => <$ty as ::holon_pbt_core::TransitionRef<
                        $crate::pbt::reference_state::ReferenceState,
                    >>::preconditions(v, state), )*
                }
            }

            fn apply_to_ref(&self, state: &mut $crate::pbt::reference_state::ReferenceState) {
                match self {
                    $( $enum_name::$variant(v) => <$ty as ::holon_pbt_core::TransitionRef<
                        $crate::pbt::reference_state::ReferenceState,
                    >>::apply_to_ref(v, state), )*
                }
            }
        }

        // SUT-side dispatch (concrete-`S`): apply_to_sut. Generic over
        // `S: SutHandle`; variants may narrow `S` to fine-grained caps —
        // `SutHandle` is a supertrait bundle of them, so they still satisfy this.
        // `SutFocusWrite` / `SutNavHistoryWrite` / `SutWatchRegister` (the caps
        // the `NavigateFocus` / `NavigateHome` / `SetupWatch` variants bind) are
        // now `SutHandle` supertraits (decomposition #4 deleted the duplicate
        // `SutHandle` copies that previously forced the method-name clash), so the
        // dispatch bound is once again just `S: SutHandle` — no extra `+` bounds.
        #[allow(async_fn_in_trait)]
        impl<S: $crate::pbt::transition_dispatch::SutHandle>
            ::holon_pbt_core::TransitionImpl<
                $crate::pbt::reference_state::ReferenceState,
                S,
            > for $enum_name {
            async fn apply_to_sut(
                &self,
                state: &$crate::pbt::reference_state::ReferenceState,
                sut: &mut S,
            ) {
                match self {
                    $( $enum_name::$variant(v) => <$ty as ::holon_pbt_core::TransitionImpl<
                        $crate::pbt::reference_state::ReferenceState,
                        S,
                    >>::apply_to_sut(v, state, sut).await, )*
                }
            }
        }

        #[cfg(feature = "otel-testing")]
        impl $crate::pbt::transition_budgets::SqlBudget for $enum_name {
            fn expected_sql(
                &self,
                state: &$crate::pbt::reference_state::ReferenceState,
            ) -> $crate::pbt::transition_budgets::ExpectedSql {
                match self {
                    $( $enum_name::$variant(v) =>
                        <$ty as $crate::pbt::transition_budgets::SqlBudget>::expected_sql(v, state), )*
                }
            }
        }

        impl $enum_name {
            /// Variant name for trace logging and Markov-weighting in
            /// generators that bias on the previous transition kind.
            pub fn variant_name(&self) -> &'static str {
                match self {
                    $( Self::$variant(_) => stringify!($variant), )*
                }
            }

            /// Value-level mirror of the type-level `TransitionFactory::required_wiring`
            /// gate used in `aggregate_transitions`. Needed at replay time so the
            /// shared engine (`stepper::run_sequence`) can decide, per concrete
            /// transition, whether a subset's wiring gates it out — turning it into
            /// a deterministic `SkippedByGating` no-op rather than re-deriving the
            /// alphabet (ADR 0009 §4).
            pub fn required_wiring(&self) -> ::holon_pbt_core::RequiredWiring {
                match self {
                    $( Self::$variant(_) => <$ty as ::holon_pbt_core::TransitionFactory<
                        $crate::pbt::reference_state::ReferenceState,
                    >>::required_wiring(), )*
                }
            }

            /// Value-level mirror of `TransitionFactory::required_caps` — the cap-analog
            /// of [`required_wiring`](Self::required_wiring), consumed at replay time so
            /// `stepper::run_sequence` can gate a concrete transition against a composed
            /// SUT's `cap_set` exactly as generation does (PCG-2).
            pub fn required_caps(&self) -> Vec<::holon_pbt_core::composition::CapId> {
                match self {
                    $( Self::$variant(_) => <$ty as ::holon_pbt_core::TransitionFactory<
                        $crate::pbt::reference_state::ReferenceState,
                    >>::required_caps(), )*
                }
            }
        }

        $vis fn aggregate_transitions(
            state: &$crate::pbt::reference_state::ReferenceState,
        ) -> ::proptest::strategy::BoxedStrategy<$enum_name> {
            use ::proptest::strategy::{Strategy, Union};
            use $crate::pbt::transition_dispatch::variant_weight_multiplier;

            let mut arms: Vec<(u32, ::proptest::strategy::BoxedStrategy<$enum_name>)> = Vec::new();

            $(
                // ADR 0007 item 3: derive the alphabet from the manifest.
                // A variant whose `RequiredWiring` the active wiring doesn't
                // satisfy is structurally absent — not even offered to the
                // generator (the dynamic `weighted_generator` would Fail it
                // anyway, but the manifest gate makes the exclusion explicit
                // and reason-free).
                if <$ty as ::holon_pbt_core::TransitionFactory<
                    $crate::pbt::reference_state::ReferenceState,
                >>::required_wiring()
                    .satisfied_by(&state.wiring)
                    && state.caps_available(&<$ty as ::holon_pbt_core::TransitionFactory<
                        $crate::pbt::reference_state::ReferenceState,
                    >>::required_caps())
                {
                    // Per-variant arm built by the shared `holon_pbt_core::weighted_arm`
                    // helper (one aggregation path across every PBT). The
                    // `HOLON_PBT_WEIGHTS` multiplier is applied there; `0` removes
                    // the variant (`Good(None)`), and rejections come back as
                    // `Fail(reasons)` for `record_rejection` to account.
                    match ::holon_pbt_core::weighted_arm::<_, $ty, $enum_name>(
                        state,
                        variant_weight_multiplier(stringify!($variant)),
                        |v| $enum_name::from(v),
                    ) {
                        ::validated::Validated::Good(Some(arm)) => arms.push(arm),
                        ::validated::Validated::Good(None) => {}
                        ::validated::Validated::Fail(reasons) => {
                            $crate::pbt::validation::record_rejection(
                                stringify!($variant),
                                &reasons,
                            );
                        }
                    }
                }
            )*

            assert!(
                !arms.is_empty(),
                "declare_e2e_transitions!: no transition applicable in state {state:?} \
                 — at least one variant must be unconditionally enabled."
            );
            Union::new_weighted(arms).boxed()
        }
    };
}

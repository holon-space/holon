//! File-per-transition pattern for the E2E PBT.
//!
//! @pbt kind transition-dispatch
//! @pbt covers transition-enum-dispatch — declare_e2e_transitions! generated
//!   E2ETransition enum: variant union caps, hand-rolled trait match, strategy
//!   aggregator (RESTRICTED file — structural recommendations in
//!   docs/Testing/PBT-Audit-2026-07-16.md, not edited here).
//!
//! Each transition kind owns its data (a struct), generation strategy
//! (`holon_pbt_core::TransitionFactory<ReferenceState>`), and behaviour
//! (`holon_pbt_core::TransitionImpl<Ref, S>`, bound on fine-grained SUT caps).
//! The `declare_e2e_transitions!` macro generates the dispatch enum, the
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
//! - There is no bundle trait. Each transition's `TransitionImpl` is bound on
//!   exactly the fine-grained SUT capability traits it drives (via
//!   `cap_transition!`); the `E2ETransition` enum's `TransitionImpl` is bound
//!   on the inlined union of every variant's caps (via
//!   `declare_e2e_transitions!`), so a variant narrowed to a single cap still
//!   satisfies it. The former coarse `SutHandle` view was deleted.

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
        // one). Concrete-`S` dispatch: generic over `S` (bound on the inlined
        // union of every variant's fine-grained caps, below), so the SUT is
        // monomorphised (the runner passes `&mut E2ESut<V>`). This is what
        // lets each variant's `TransitionImpl<R, S>` impl narrow `S` to just
        // the one capability it needs — that narrower bound is implied by the
        // enum impl's union, so a variant bound on `S: SutEditorMirrorWrite`
        // still satisfies this enum impl.
        // Ref-side dispatch (S-independent): preconditions + apply_to_ref.
        impl ::holon_pbt_core::TransitionRef<$crate::pbt::reference_state::ReferenceState>
            for $enum_name {
            type Reason = ::holon_pbt_core::validation::Reason;

            fn preconditions(
                &self,
                state: &$crate::pbt::reference_state::ReferenceState,
            ) -> ::validated::Validated<(), ::holon_pbt_core::validation::Reason> {
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
                // E-solid shadow-mesh centralized primary catch-up: after every
                // ref transition, mirror the ref block map into the shadow
                // primary at the SUT's fed Lamport height. One site covers all
                // machines dispatching this enum; a variant the mirror misses
                // self-heals at the next catch-up. No-op until the mesh exists.
                state.shadow_catch_up_primary();
            }
        }

        // SUT-side dispatch (concrete-`S`): apply_to_sut. Generic over `S`
        // bound on the inlined union (below) of every variant's fine-grained
        // caps; a variant may narrow `S` to just the cap it needs and still
        // satisfy this union. The union is spelled out explicitly here rather
        // than behind a single `SutHandle` supertrait bundle (which was
        // deleted), so adding a cap to one variant surfaces as one `+ Sut…`
        // line here — the single place the enum's cap requirements live.
        #[allow(async_fn_in_trait)]
        impl<S: ::holon_pbt_core::capabilities::SutEditorMirrorWrite
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
            + ::holon_pbt_core::capabilities::SutMutate
            + ::holon_pbt_core::capabilities::SutSeamMutate
            + ::holon_pbt_core::capabilities::SutBlockCreate
            + ::holon_pbt_core::capabilities::SutTemplateInstantiate
            + ::holon_pbt_core::capabilities::SutBlockToPage
            + ::holon_pbt_core::capabilities::SutPageIdentity
            + ::holon_pbt_core::capabilities::SutClockAdvance
            + ::holon_pbt_core::capabilities::SutFixtureFs
            + ::holon_pbt_core::capabilities::SutAppLifecycle
            + $crate::pbt::transitions::apply_mutation::SutApplyMutation
            + $crate::pbt::transitions::start_app::SutBootWatches>
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
            fn expected_sql<R: ::holon_pbt_core::capabilities::RefSqlCardinality>(
                &self,
                state: &R,
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
                    .satisfied_by(&state.harness.wiring)
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
                            ::holon_pbt_core::validation::record_rejection(
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

/// Confined proof that the generic-over-`R` `cap_transition!` arm expands and
/// type-checks. An *uninvoked* macro arm is unchecked, so this dummy transition
/// exercises it (generic `R` bound + `sql_budget:` clause) WITHOUT touching any
/// real transition file. Asserts `declared_caps()` single-sources the cap and,
/// under `otel-testing`, that the `sql_budget:` clause emits a working generic
/// `SqlBudget::expected_sql`.
#[cfg(test)]
mod cap_transition_generic_arm_selftest {
    use ::holon_pbt_core::capabilities::RefBlockTree;
    use ::holon_pbt_core::capabilities::SutBlockTreeWrite;
    use ::holon_pbt_core::composition::CapId;

    #[derive(Clone, Debug)]
    struct DummyGenericTransition;

    crate::cap_transition! {
        DummyGenericTransition: SutBlockTreeWrite,
        where R: [RefBlockTree],
        |_me, _state, _sut| { /* generic over R + S: SutBlockTreeWrite; body unused */ }
        sql_budget: |_me, state| {
            crate::pbt::transition_budgets::ExpectedSql {
                reads: state.block_count() + 10,
                writes: 2,
                ddl: 0,
                tolerance: 1,
            }
        }
    }

    #[test]
    fn declared_caps_single_sources_the_cap() {
        assert_eq!(
            DummyGenericTransition::declared_caps(),
            vec![CapId::of::<dyn SutBlockTreeWrite>()],
        );
    }

    #[cfg(feature = "otel-testing")]
    #[test]
    fn sql_budget_clause_emits_generic_expected_sql() {
        use ::holon_pbt_core::capabilities::RefSqlCardinality;

        use crate::pbt::transition_budgets::SqlBudget;

        struct DummyRef;
        impl RefSqlCardinality for DummyRef {
            fn block_count(&self) -> usize {
                3
            }
            fn document_count(&self) -> usize {
                1
            }
            fn active_watch_count(&self) -> usize {
                0
            }
            fn last_navigate_first_visit(&self) -> bool {
                false
            }
        }

        let sql = DummyGenericTransition.expected_sql(&DummyRef);
        assert_eq!(sql.reads, 13); // block_count (3) + 10
        assert_eq!(sql.writes, 2);
        assert_eq!(sql.ddl, 0);
        assert_eq!(sql.tolerance, 1);
    }
}

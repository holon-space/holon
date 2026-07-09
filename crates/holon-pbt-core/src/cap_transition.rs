//! `cap_transition!` — the shared, reference-state-agnostic transition macro.
//!
//! Lives in `holon-pbt-core` (not the integration-test crate) so every PBT
//! consumer — the central keystone crate AND the per-subsystem companion crates
//! (`holon-loro-testing`, …) — expands the same macro. A companion crate cannot
//! depend on `holon-integration-tests` (that crate depends on *it*), so the
//! macro's home has to be a crate both share: this one.
//!
//! All three arms emit a `TransitionImpl<R, Sut>` generic over the reference
//! type `R` — none pins a concrete reference state. The optional `sql_budget:`
//! clause emits the `#[cfg(feature = "otel-testing")]`
//! [`crate::budget::SqlBudget`] impl, single-sourcing behaviour, cap, and SQL
//! budget at one call site.

/// `cap_transition!` — terse, single-sourced SUT dispatch + cap declaration.
///
/// Generates a transition's `TransitionImpl<R, S>` block (bound on
/// exactly one fine-grained cap) AND the matching `declared_caps()` from the
/// **same** cap token, so the dispatch bound and
/// `TransitionFactory::required_caps` can no longer drift. Any transition
/// authored through this macro no longer needs an entry in the
/// `required_caps_match_transition_impl_bounds` guard test.
///
/// It is also the migration seam (see `docs/Testing/PbtCompositionDesign.md`
/// §8.9): the per-transition call site is agnostic to whether `apply_to_sut`
/// dispatches over a generic `S: Cap` (today) or a concrete composed `CapMap`
/// (post-E5) — flipping that is a change to *this macro's expansion*, not to
/// any transition file. The body calls `sut.<cap-method>(…)`, which works
/// identically whether `sut: &mut S` (S: Cap) or `sut: &mut CapMap` (CapMap
/// implements every cap via `#[capmap_adapter]`).
///
/// Single-cap form (the `TransitionFactory::required_caps` body becomes
/// `Self::declared_caps()`). The `TransitionImpl` is generic over an
/// unbounded reference type `R` — the body ignores `state`:
/// ```ignore
/// cap_transition! {
///     SplitBlock: SutBlockTreeWrite,
///     |me, _state, sut| { sut.apply_split_block(&me.block_id, me.position).await; }
/// }
/// ```
///
/// No-cap form (generic, unbounded `S` and `R`; `required_caps` stays the
/// trait default of empty):
/// ```ignore
/// cap_transition! { Nothing, |_me, _state, _sut| {} }
/// ```
///
/// Generic-over-reference form (+ optional SQL budget): makes `TransitionImpl`
/// generic over the reference type `R` (bounded by the caller's bracketed
/// `Ref*` list) and folds in the transition's SQL budget via the optional
/// `sql_budget:` clause. The `Ref*` bounds go in brackets
/// (`where R: [Bound + Bound]`) so the list may be `+`-joined — a bare
/// `:path` fragment can't be followed by `+`. The `sql_budget:` lambda
/// body becomes the transition's
/// `#[cfg(feature = "otel-testing")] impl SqlBudget::expected_sql` (its
/// `state` is `&impl RefSqlCardinality`); omit the clause for a SQL-free
/// transition. `declared_caps()` is single-sourced from `$cap` exactly as
/// in the single-cap arm.
/// ```ignore
/// cap_transition! {
///     SplitBlock: SutBlockTreeWrite,
///     where R: [RefBlockTree + RefFocus],
///     |me, _state, sut| { sut.apply_split_block(&me.block_id, me.position).await; }
///     sql_budget: |_me, state| {
///         let blocks = state.block_count();
///         ExpectedSql { reads: blocks + 5, writes: 2, ddl: 0, tolerance: 1 }
///     }
/// }
/// ```
#[macro_export]
macro_rules! cap_transition {
    // ── single cap (generic, unbounded R) ────────────────────────────
    (
        $name:ident : $cap:path,
        | $me:ident, $state:ident, $sut:ident | $body:block
    ) => {
        impl $name {
            /// Single source of this transition's cap: `required_caps()` forwards
            /// here, and the `TransitionImpl` bound below binds the same `$cap`.
            pub(crate) fn declared_caps() -> ::std::vec::Vec<$crate::composition::CapId> {
                ::std::vec![$crate::composition::CapId::of::<dyn $cap>()]
            }
        }

        #[allow(async_fn_in_trait)]
        impl<R, S: $cap> $crate::TransitionImpl<R, S> for $name {
            async fn apply_to_sut(&self, $state: &R, $sut: &mut S) {
                let $me = self;
                $body
            }
        }
    };

    // ── generic over the reference type `R` (+ optional SQL budget) ──
    //
    // Emits a `TransitionImpl<R, S>` generic over the reference type `R`
    // (bounded by the caller's bracketed `Ref*` list), plus — when the
    // optional `sql_budget:` clause is present — the matching
    // `#[cfg(feature = "otel-testing")] impl SqlBudget`. Behaviour, cap,
    // and SQL budget are single-sourced at one call site. The `R` bounds
    // are captured as bracketed token-trees so the list may use `+`
    // (a `:path` fragment can't be followed by `+`).
    (
        $name:ident : $cap:path,
        where R : [ $( $rbound:tt )+ ] $(,)?
        | $me:ident, $state:ident, $sut:ident | $body:block
        $(
            sql_budget: | $bme:ident, $bstate:ident | $budget:block
        )?
    ) => {
        impl $name {
            /// Single source of this transition's cap: `required_caps()` forwards
            /// here, and the generic `TransitionImpl` bound below binds the same `$cap`.
            pub(crate) fn declared_caps() -> ::std::vec::Vec<$crate::composition::CapId> {
                ::std::vec![$crate::composition::CapId::of::<dyn $cap>()]
            }
        }

        #[allow(async_fn_in_trait)]
        impl<R: $( $rbound )+, S: $cap> $crate::TransitionImpl<R, S> for $name {
            async fn apply_to_sut(&self, $state: &R, $sut: &mut S) {
                let $me = self;
                $body
            }
        }

        $(
            #[cfg(feature = "otel-testing")]
            impl $crate::budget::SqlBudget for $name {
                fn expected_sql<R2: $crate::capabilities::RefSqlCardinality>(
                    &self,
                    $bstate: &R2,
                ) -> $crate::budget::ExpectedSql {
                    let $bme = self;
                    $budget
                }
            }
        )?
    };

    // ── no cap (generic, unbounded S and R) ──────────────────────────
    (
        $name:ident,
        | $me:ident, $state:ident, $sut:ident | $body:block
    ) => {
        #[allow(async_fn_in_trait)]
        impl<R, S> $crate::TransitionImpl<R, S> for $name {
            async fn apply_to_sut(&self, $state: &R, $sut: &mut S) {
                let $me = self;
                $body
            }
        }
    };
}

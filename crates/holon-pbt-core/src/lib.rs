//! @c4 component
//! @c4 layer Testing
//! Pattern: Strategy
//! @c4 uses holon-api "shared value & operation types" "Rust"
//!
//! Cross-PBT transition traits shared between `holon-layout-testing` and
//! `holon-integration-tests`.
//!
//! Defines the two PBT transition traits — `TransitionFactory<Ref>` and
//! `TransitionImpl<Ref, Sut>` — generic over reference-state and SUT,
//! plus the variant structs that show up in more than one PBT.
//!
//! ## Why generic
//!
//! Each PBT carries its own reference-state and its own SUT handle.
//! The layout PBT has a thin generation context + a frontend
//! `LayoutSut`; the integration-tests PBT has a rich `ReferenceState` +
//! an async `dyn SutHandle`. By parameterising the traits over `Ref`
//! and `Sut`, the *same* variant struct (e.g. [`SwitchViewMode`]) can
//! have one impl per PBT — different `(Ref, Sut)` tuples → distinct
//! impls → no coherence conflict.
//!
//! ## File per variant
//!
//! Every variant under [`interactions`] gets its own file holding the
//! struct. The per-PBT impls of `TransitionFactory` / `TransitionImpl`
//! live in the consumer crate's own per-variant file (orphan rule
//! permits this — at least one of the type parameters is local to the
//! implementing crate).

// Lets `#[holon_macros::capmap_adapter]`-generated impls use the absolute path
// `::holon_pbt_core::composition::CapMap` even when expanded *inside* this
// crate — so the same macro works for cap traits hosted in domain crates (the
// cap home-rule).
extern crate self as holon_pbt_core;

use proptest::strategy::BoxedStrategy;
use validated::Validated;

pub mod attribution;
pub mod bisect;
pub mod block_compare;
pub mod budget;
pub mod caching_proxy;
pub mod cap_transition;
pub mod capabilities;
pub mod component_set;
pub mod composition;
pub mod content_generators;
pub mod contribution;
pub mod correspondence;
pub mod fixture;
pub mod interactions;
pub mod invariant;
pub mod null_ref;
pub mod observables;
pub mod panic_filter;
pub mod retry;
pub mod sibling_order;
pub mod types;
pub mod validation;
pub mod wiring;

pub use bisect::Localization;
pub use bisect::bisect;
pub use bisect::bisect_downward;
pub use bisect::bisect_upward;
pub use caching_proxy::CachingProxy;
pub use caching_proxy::cached;
pub use component_set::Component;
pub use component_set::ComponentSet;
pub use component_set::ComponentSetError;
pub use component_set::Projection;
pub use contribution::CapInstaller;
pub use contribution::CrateId;
pub use contribution::GeneratorFactory;
pub use contribution::PbtContribution;
pub use contribution::PbtFootprint;
pub use contribution::fold_catalog;
pub use correspondence::Converge;
pub use correspondence::Correspondence;
pub use correspondence::Extraction;
pub use correspondence::NamedCompare;
pub use correspondence::Observable;
pub use correspondence::StoreProjection;
pub use interactions::DeliverBlockContent;
pub use interactions::SwitchViewMode;
pub use interactions::ToggleCollapse;
pub use interactions::ToggleDrawer;
pub use invariant::Invariant;
pub use invariant::InvariantId;
pub use invariant::InvariantResult;
pub use invariant::RunMode;
pub use observables::NonSeedBlocks;
pub use observables::ref_non_seed_blocks;
pub use sibling_order::compare_sibling_order;
pub use wiring::Actor;
pub use wiring::RequiredWiring;
pub use wiring::StorageAdapter;
pub use wiring::SyncAdapter;
pub use wiring::Wiring;
pub use wiring::WiringError;
pub use wiring::any_valid_wiring;
pub use wiring::wiring_axes;
pub use wiring::wiring_from_exact_spec;

/// Static contract for *creating* a transition. Each variant struct
/// implements this once per PBT, parameterised by that PBT's
/// reference-state type. Returns `Good((weight, strategy))` if the
/// variant applies in `state`, or `Fail(reasons)` so the runner can
/// account for *why* the variant was rejected.
pub trait TransitionFactory<Ref>: Sized {
    type Reason;
    fn weighted_generator(state: &Ref) -> Validated<(u32, BoxedStrategy<Self>), Self::Reason>;

    /// Structural wiring requirement (ADR 0007 item 3). The transition is
    /// excluded from a manifest's alphabet unless this is satisfied. It is
    /// **necessary, not sufficient** — `weighted_generator` still gates
    /// dynamically (e.g. "a block exists to edit"); the manifest gates
    /// structurally. Default: no requirement.
    ///
    /// Ref-independent in practice (the same transition type needs the same
    /// wiring regardless of which reference model drives it), but declared
    /// here so the generator can read it off the factory it already names.
    fn required_wiring() -> crate::wiring::RequiredWiring {
        crate::wiring::RequiredWiring::Any
    }

    /// Capability requirement: the cap-analog of
    /// [`required_wiring`](Self::required_wiring). Where `required_wiring`
    /// gates the alphabet on a manifest's *subsystem wiring*, this gates it
    /// on the *providers a composed `CapMap` actually supplies* — a transition
    /// is excluded from a composed SUT's alphabet unless every cap here is
    /// present in the
    /// map's [`cap_set`](crate::composition::CapMap::cap_set). Each entry is
    /// the capability this transition's [`TransitionImpl`] impl is bound on
    /// (e.g. `SplitBlock` → `[CapId::of::<dyn SutBlockTreeWrite>()]`), so a
    /// partial config (e.g. Loro-only) drops the transitions whose
    /// `apply_to_sut` caps it lacks rather than panicking on an absent
    /// `expect`. **Necessary, not sufficient** — `weighted_generator` still
    /// gates dynamically. Default: no requirement (always cap-feasible).
    fn required_caps() -> Vec<crate::composition::CapId> {
        Vec::new()
    }
}

/// Build one weighted arm of a transition `Union` from a single
/// [`TransitionFactory`] variant. This is the shared core every PBT's
/// generator repeats: call the factory, apply a per-variant weight
/// multiplier, drop zero-weight arms, and `prop_map` the generated
/// variant struct into the caller's transition enum.
///
/// Returns:
/// - `Good(Some((weight, strategy)))` — the variant applies; push it.
/// - `Good(None)` — applies but its effective weight is 0; skip it.
/// - `Fail(reasons)` — the variant rejected; the caller decides whether to
///   record the reasons (the wide PBT does via `record_rejection`, the
///   layout/pure slices discard them).
///
/// Pass `weight_multiplier = 1` when no per-variant tuning applies.
/// Returning `Validated` (rather than taking a rejection callback) keeps
/// this crate free of the `NEVec` payload type while letting callers
/// `Fail(reasons) => …` with the original reasons in hand.
pub fn weighted_arm<R, F, T>(
    state: &R,
    weight_multiplier: u32,
    into: impl Fn(F) -> T + 'static,
) -> Validated<Option<(u32, BoxedStrategy<T>)>, F::Reason>
where
    F: TransitionFactory<R> + std::fmt::Debug + 'static,
    T: std::fmt::Debug + 'static,
{
    use proptest::strategy::Strategy;
    match F::weighted_generator(state) {
        Validated::Good((w, s)) => {
            let final_weight = w.saturating_mul(weight_multiplier);
            Validated::Good((final_weight > 0).then(|| (final_weight, s.prop_map(into).boxed())))
        }
        Validated::Fail(reasons) => Validated::Fail(reasons),
    }
}

/// Ref-side per-variant behaviour: preconditions against the reference
/// model + apply-to-reference. Generic over `Ref` only — **independent
/// of the SUT type** — so S-less contexts (a `TransitionFactory`'s
/// generator, the proptest state-machine driver) can call these without
/// naming a SUT.
///
/// Split out of `TransitionImpl` (which keeps only the SUT-parameterised
/// `apply_to_sut`) so concrete-`S` dispatch works: a transition whose
/// `apply_to_sut` is bound on fine-grained capabilities still has its
/// ref logic defined once, callable from anywhere.
pub trait TransitionRef<Ref>: Clone + std::fmt::Debug + Send + Sync {
    type Reason;
    fn preconditions(&self, state: &Ref) -> Validated<(), Self::Reason>;
    fn apply_to_ref(&self, state: &mut Ref);
}

/// SUT-side per-variant behaviour: drive the system under test. Generic
/// over `Ref` and `Sut` (the PBT's SUT handle, concrete or `dyn`). The
/// `Sut` bound is where a transition declares which SUT capabilities it
/// needs, so the same struct runs on any SUT supplying them.
///
/// `Sut: ?Sized` so impls can be written against `dyn SomeTrait` as well
/// as concrete types.
#[allow(async_fn_in_trait)]
pub trait TransitionImpl<Ref, Sut: ?Sized> {
    async fn apply_to_sut(&self, state: &Ref, sut: &mut Sut);
}

//! Cross-PBT primitives shared between `holon-layout-testing` and
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

use proptest::strategy::BoxedStrategy;
use validated::Validated;

pub mod caching_proxy;
pub mod capabilities;
pub mod fixture;
pub mod interactions;
pub mod invariant;

pub use caching_proxy::{cached, CachingProxy};
pub use invariant::{Invariant, InvariantId, InvariantResult, RunMode};

pub use interactions::{DeliverBlockContent, SwitchViewMode, ToggleCollapse, ToggleDrawer};

/// Static contract for *creating* a transition. Each variant struct
/// implements this once per PBT, parameterised by that PBT's
/// reference-state type. Returns `Good((weight, strategy))` if the
/// variant applies in `state`, or `Fail(reasons)` so the runner can
/// account for *why* the variant was rejected.
pub trait TransitionFactory<Ref>: Sized {
    type Reason;
    fn weighted_generator(state: &Ref) -> Validated<(u32, BoxedStrategy<Self>), Self::Reason>;
}

/// Per-variant behaviour: preconditions against the reference model,
/// apply-to-reference, apply-to-SUT. Generic over both `Ref` (the PBT's
/// reference-state type) and `Sut` (the PBT's SUT handle, which may be
/// a concrete struct or `dyn SomeTrait`).
///
/// `Sut: ?Sized` so impls can be written against `dyn SutHandle` as
/// well as concrete types.
#[allow(async_fn_in_trait)]
pub trait TransitionImpl<Ref, Sut: ?Sized>: Clone + std::fmt::Debug + Send + Sync {
    type Reason;
    fn preconditions(&self, state: &Ref) -> Validated<(), Self::Reason>;
    fn apply_to_ref(&self, state: &mut Ref);
    async fn apply_to_sut(&self, state: &Ref, sut: &mut Sut);
}

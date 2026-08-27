//! The derived Petri-net projection (ADR 0032 §2): rule blocks and operation
//! descriptors compiled into one read-only net, plus the conflict and cycle
//! analyses that run over it.
//!
//! Everything here is a pure function of its sources, which is what makes the
//! net a derived artifact and never a second authority (ADR 0032 §2, ADR 0024
//! P1a). It carries no state beyond the current derivation, so it cannot go
//! stale whether a caller pulls it on demand or holds it as a reactive var
//! recomputed on source change — this crate stays pure under either delivery.
//!
//! Petri-net vocabulary — transition, place, arc, marking — lives in this
//! crate only. Dispatch keeps its `Operation*` names; [`bridge`] is the one
//! place the two vocabularies meet (ADR 0032 §8).

pub mod analysis;
pub mod bridge;
pub mod compile;
pub mod guards;
pub mod net;

pub use analysis::ConflictReport;
pub use analysis::CycleReport;
pub use analysis::conflicts;
pub use analysis::cycles;
pub use bridge::NetEntity;
pub use bridge::NetError;
pub use bridge::TransitionKey;
pub use bridge::TransitionSource;
pub use compile::RuleAcceptance;
pub use compile::RuleSource;
pub use compile::derive_net;
pub use net::Analyzability;
pub use net::ArcOrigin;
pub use net::Aspect;
pub use net::BindingVar;
pub use net::CompiledNet;
pub use net::Flow;
pub use net::GuardResidue;
pub use net::NetArc;
pub use net::NetCompileError;
pub use net::NetTransition;
pub use net::UndeclaredHalf;

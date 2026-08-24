//! The derived Petri-net projection (ADR 0032 §2): rule blocks and operation
//! descriptors compiled into one read-only net, plus the conflict and cycle
//! analyses that run over it.
//!
//! Everything here is a pure function of its sources. The net is never
//! stored, so it cannot go stale — it is a derived artifact, never a second
//! authority (ADR 0032 §2, ADR 0024 P1a).
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
pub use bridge::TransitionSource;
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

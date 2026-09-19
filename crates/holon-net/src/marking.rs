//! The durable state an enabledness query reads.
//!
//! A token exists because a durable row exists with the attributes it has
//! (`docs/adr/0032-petri-net-execution-semantics.md` §1). Enabledness asks
//! only one thing of that state, so the trait carries only that.
//!
//! Read-only: advancing a net is the simulation marking's job
//! (`holon_engine::Marking`), whose write half has no counterpart here.

use holon_api::EntityUri;
use holon_pattern::arcs::ArcRelation;

pub trait Marking {
    /// Whether `entity`'s row exists in `relation` — the existence token,
    /// which is read and produced but never consumed.
    fn present(&self, relation: &ArcRelation, entity: &EntityUri) -> bool;
}

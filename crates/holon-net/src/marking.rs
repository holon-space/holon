//! The durable state an enabledness query reads.
//!
//! A token exists because a durable row exists with the attributes it has
//! (`docs/adr/0032-petri-net-execution-semantics.md` §1). Three questions are
//! enough to decide every arc the language can express: does the subject's row
//! exist, what does a cell of it hold, and which entities does a hop reach.
//!
//! Read-only: advancing a net is the simulation marking's job
//! (`holon_engine::Marking`), whose write half has no counterpart here.

use holon_api::EntityUri;
use holon_pattern::Value;
use holon_pattern::arcs::ArcPlace;
use holon_pattern::arcs::ArcRelation;

pub trait Marking {
    /// Whether `entity`'s row exists in `relation` — the existence token,
    /// which is read and produced but never consumed.
    fn present(&self, relation: &ArcRelation, entity: &EntityUri) -> bool;

    /// The cell an arc's refinement tests. `None` when the row or the column
    /// holds nothing, which no refinement satisfies.
    ///
    /// **An empty cell is `None`.** Reporting it as `Some(Value::Null)` is a
    /// contract violation, not a second spelling: `Null` compares equal to
    /// `Null`, so a hop would correlate every row with an empty cell into one
    /// family. `holon_net::enabledness` asserts on it rather than letting an
    /// implementor decide a verdict this way.
    fn value(&self, place: &ArcPlace, entity: &EntityUri) -> Option<Value>;

    /// One hop step: every entity whose `to` cell holds `value`.
    ///
    /// The only method with a cost — measured at 1.229 ms p50 over 3255 blocks
    /// (D150.c increment 0), about the price of a primary-key lookup.
    fn matching(&self, to: &ArcPlace, value: &Value) -> Vec<EntityUri>;
}

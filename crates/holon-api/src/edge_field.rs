//! Block **edge fields** — the junction-backed, set-valued block attributes
//! (`tags`, `requires`) that are projected to their own tables (`block_tags`,
//! `block_requires`) rather than living as scalar columns on `block_raw`.
//!
//! [`EdgeFieldUpdate`] names the edge-field category as a *type* so a write to
//! one edge field is parameterized over *which* field — neither `tags` nor
//! `requires` is special-cased. (The production projection still hand-lists the
//! edge fields in several places; unifying those behind this category so that
//! omitting one becomes unrepresentable is a tracked follow-up.)

use crate::types::Tags;
use crate::EntityUri;

/// An update to exactly one of a block's edge fields. Carried as a typed value
/// (not a stringly-typed map) so the boundary parse happens once, at
/// construction, per the parse-don't-validate rule.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum EdgeFieldUpdate {
    /// Replace the block's `tags` set (the `block_tags` junction).
    Tags(Tags),
    /// Replace the block's `requires` dependency edges (the `block_requires`
    /// junction): the block ids this block depends on / is blocked by.
    Requires(Vec<EntityUri>),
}

impl EdgeFieldUpdate {
    /// The edge field this update targets, for labelling / display.
    pub fn field_name(&self) -> &'static str {
        match self {
            EdgeFieldUpdate::Tags(_) => "tags",
            EdgeFieldUpdate::Requires(_) => "requires",
        }
    }
}

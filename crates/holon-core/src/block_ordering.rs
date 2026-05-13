//! `BlockOrdering` — encapsulate positional intent for block aggregates.
//!
//! Chord ops express "place this block under `parent`, after `after_id`"
//! as a typed intent. The trait hides the SqlOnly-mode legacy shape
//! (`gen_key_between` + paired `set_field("parent_id") +
//! set_field("sort_key", X)`) from chord-op call sites. Loro-backed
//! implementations route the intent through Loro's tree.mov_after; SqlOnly
//! implementations compute a fractional index and persist it in
//! `block.sort_key` directly.
//!
//! After this trait lands, `gen_key_between` is no longer referenced from
//! `holon-core::traits` — chord ops are purely positional.

use async_trait::async_trait;
use holon_api::EntityUri;

use crate::traits::Result;

/// Provider of positional-intent writes for block aggregates.
///
/// Implementations:
/// - `LoroBlockOrdering` (in `holon`) — routes `place` through the cell
///   registry's `write_position`, which calls
///   `LoroBackend::update_block_position` (tree.mov_after). Loro generates
///   the resulting fractional index; new block sort_keys come back as
///   placeholders that the outbound projector overwrites.
/// - `SqlBlockOrdering` (in `holon`) — runs `gen_key_between` against the
///   neighbor SQL `block.sort_key` values and emits paired `set_field`
///   writes via the underlying `OperationProvider`. The fractional-index
///   string is what the SQL column persists.
#[async_trait]
pub trait BlockOrdering: Send + Sync {
    /// Place `uri` under `parent_id` immediately after `after_id` (or
    /// first when `None`).
    async fn place(&self, uri: &EntityUri, parent_id: &str, after_id: Option<&str>) -> Result<()>;

    /// Compute the sort_key value for a NEW block being created under
    /// `parent_id`, immediately after `after_id`. In Loro mode this
    /// returns a placeholder (Loro overwrites once `apply_create` reads
    /// `Event::position_after_block_id`); in SqlOnly mode this returns
    /// the `gen_key_between` value to persist verbatim in
    /// `block.sort_key`.
    async fn new_child_anchor(&self, parent_id: &str, after_id: Option<&str>) -> Result<String>;

    /// Block id immediately preceding `id` among its siblings (same
    /// parent, strictly-lower sort_key, maximal under that constraint).
    /// `None` when `id` is the first child or has no block parent.
    async fn prev_sibling(&self, id: &str) -> Result<Option<String>>;

    /// Block id immediately following `id` among its siblings (same
    /// parent, strictly-higher sort_key, minimal under that constraint).
    /// `None` when `id` is the last child or has no block parent.
    async fn next_sibling(&self, id: &str) -> Result<Option<String>>;

    /// Block id of the first child of `parent_id` (lowest sort_key).
    /// `None` when `parent_id` has no children.
    async fn first_child(&self, parent_id: &str) -> Result<Option<String>>;

    /// Block id of the last child of `parent_id` (highest sort_key).
    /// `None` when `parent_id` has no children.
    async fn last_child(&self, parent_id: &str) -> Result<Option<String>>;

    /// All children of `parent_id` in positional order (low → high
    /// sort_key in SqlOnly mode; Loro tree order in Loro mode).
    /// Returns an empty Vec when there are no children.
    ///
    /// This is the authoritative ordering the system actually renders /
    /// projects from. Use it as the live-side ground truth in
    /// assertions instead of computing order from a `Block`'s
    /// `sort_key` / `sequence` field — those are encoding-specific.
    async fn children(&self, parent_id: &str) -> Result<Vec<String>>;
}

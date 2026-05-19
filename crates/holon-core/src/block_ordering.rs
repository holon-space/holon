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
use holon_api::{BlockContent, EntityUri, Tags};

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

    /// Synchronously create `new_id` in the authoritative tree as a child of
    /// `parent_id`, positioned after `after_id` (or first when `None`).
    ///
    /// Returns `true` when handled by the Loro backing (the block is now in
    /// the tree and the SQL row follows via the outbound projector), `false`
    /// in SqlOnly mode (default) so the caller uses its SQL create path.
    ///
    /// Why this exists: the OrgMode initial scan creates parser blocks in SQL
    /// and then calls [`place`](Self::place), which needs the block already in
    /// the Loro tree. The inbound EventBus consumer that would mirror it in is
    /// not running yet during the first scan (it starts post-scan), so without
    /// a synchronous create `place` fails with `Block not found`. Callers must
    /// invoke this **parent-first**: `create_block` resolves the parent in the
    /// tree and errors if it is absent.
    ///
    /// `content` carries the full typed content (text vs source + language),
    /// not a bare string — org-parsed `#+BEGIN_SRC` blocks must land in the
    /// tree as `BlockContent::Source`, else the outbound projector writes
    /// `content_type = text` back over the parser's `source` and every source
    /// block silently degrades to text.
    async fn create_in_tree(
        &self,
        _: &EntityUri,
        _: Option<&EntityUri>,
        _: &EntityUri,
        _: BlockContent,
        _: &std::collections::HashMap<String, holon_api::Value>,
        _: &Tags,
        _: &[String],
    ) -> Result<bool> {
        Ok(false)
    }

    /// True when block writes are Loro-authoritative — i.e. the outbound
    /// projector (`LoroSyncController::on_loro_changed`) is the sole writer of
    /// the SQL `block_raw` row. False in SqlOnly mode, where the org reconciler
    /// must create rows directly via the command bus. Org ingestion uses this
    /// to skip the redundant command-bus block *create* in Loro mode: the
    /// block lands in Loro via `create_in_tree` and the projector writes the
    /// row, eliminating the dual-writer race on `sort_key`/`properties`.
    fn is_loro_backed(&self) -> bool {
        false
    }

    /// Apply a block **update** intent against the authoritative tree.
    ///
    /// `params` is the flat field map the org reconciler used to build the
    /// command-bus `"update"` batch (`build_block_params` shape): an `id`,
    /// the changed content/edge/scalar fields, an optional
    /// `POSITION_AFTER_BLOCK_ID_PARAM`, and the `ROUTING_DOC_URI_KEY` hint.
    ///
    /// Loro-backed impls route each field through `set_field` (→ Loro; the
    /// outbound projector writes the SQL row) and the position through
    /// [`place`](Self::place). SqlOnly impls write the SQL row directly,
    /// choosing the `create`/`update` op so the CDC event kind matches the
    /// row's prior presence (the cache subscriber distinguishes the two).
    ///
    /// This is the single org→block write seam for mutations — there is no
    /// command bus behind it.
    async fn update_in_tree(
        &self,
        params: std::collections::HashMap<String, holon_api::Value>,
    ) -> Result<()>;

    /// Apply a block **delete** intent against the authoritative tree.
    ///
    /// `params` carries the `id` and the `ROUTING_DOC_URI_KEY` hint (so the
    /// SQL `prepare_delete` can skip the recursive document walk). Loro-backed
    /// impls delete from Loro (the outbound projector emits the SQL delete);
    /// SqlOnly impls delete the SQL row directly.
    async fn delete_in_tree(
        &self,
        params: std::collections::HashMap<String, holon_api::Value>,
    ) -> Result<()>;

    /// All children of `parent_id` in positional order (low → high
    /// sort_key in SqlOnly mode; Loro tree order in Loro mode).
    /// Returns an empty Vec when there are no children.
    ///
    /// This is the authoritative ordering the system actually renders /
    /// projects from. Use it as the live-side ground truth in
    /// assertions instead of computing order from a `Block`'s
    /// `sort_key` / `sequence` field — those are encoding-specific.
    async fn children(&self, parent_id: &str) -> Result<Vec<String>>;

    /// Project the authoritative order key (Loro fractional index) to the
    /// SQL `sort_key` sink for `ids`. A block created but never repositioned
    /// emits no Loro mov delta, so the outbound projector never writes its fi
    /// to SQL and it keeps the default `"A0"`, mis-sorting against moved
    /// siblings (real fi). The org-scan reconciler calls this after its place
    /// loop so freshly-created-but-unmoved blocks get a real `sort_key`.
    /// Default + SqlOnly: no-op (SQL itself owns `sort_key` there).
    async fn project_sort_keys(&self, _: &[&str]) -> Result<()> {
        Ok(())
    }
}

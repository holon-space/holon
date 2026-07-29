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
use holon_api::BlockContent;
use holon_api::EntityUri;
use holon_api::Tags;
use holon_api::capability::Consolidator;

use crate::traits::Result;

/// One block-create intent for [`BlockOrdering::create_in_tree_batch`] — the
/// same payload [`BlockOrdering::create_in_tree`] takes, minus the positional
/// anchor: a batch creates its blocks in request order and the caller's place
/// pass owns their final positions.
#[derive(Debug, Clone)]
pub struct BlockCreateRequest {
    pub parent_id: EntityUri,
    pub id: EntityUri,
    pub content: BlockContent,
    pub properties: std::collections::HashMap<String, holon_api::Value>,
    pub tags: Tags,
    pub requires: Vec<EntityUri>,
    pub advice_suppressed: Vec<EntityUri>,
}

/// Provider of positional-intent writes for block aggregates.
///
/// Implementations:
/// - `LoroBlockOrdering` (in `holon`) — routes `place` through the cell
///   registry's `write_position`, which calls
///   `LoroBackend::update_block_position` (tree.mov_after). Loro generates the
///   resulting fractional index; new block sort_keys come back as placeholders
///   that the outbound projector overwrites.
/// - `SqlBlockOrdering` (in `holon`) — runs `gen_key_between` against the
///   neighbor SQL `block.sort_key` values and emits paired `set_field` writes
///   via the underlying `OperationProvider`. The fractional-index string is
///   what the SQL column persists.
#[async_trait]
pub trait BlockOrdering: Send + Sync {
    /// Place `uri` under `parent_id` immediately after `after_id` (or
    /// first when `None`).
    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> Result<()>;

    /// Realize the **total** sibling order of `parent_id` as exactly
    /// `ordered_ids` (already children of `parent_id`, in the intended
    /// document order). The order owner mints a fresh, gap-free key
    /// sequence so every sink stores it verbatim — closing the
    /// projection-totality gap that incremental [`place`](Self::place)
    /// leaves when an owner is handed a *full reorder* (the org re-ingest
    /// case: the file's line order is a complete order, and one-at-a-time
    /// `after_sibling` inserts against a mutating store don't reliably
    /// converge to it). See `Replication.md` §5/§11 (one owner, fractional
    /// index, projected verbatim).
    ///
    /// Default: incremental `place` of each id after its predecessor —
    /// correct for tree-backed owners (Loro) where `place` is idempotent
    /// and reads live neighbors. The SQL (no-Loro) order owner overrides
    /// this with a single positional `gen_n_keys` assignment, which is
    /// total by construction. Does not re-parent beyond setting
    /// `parent_id` to `parent_id` (the position write `place` already does).
    async fn place_all(&self, parent_id: &EntityUri, ordered_ids: &[EntityUri]) -> Result<()> {
        let mut prev: Option<&EntityUri> = None;
        for id in ordered_ids {
            self.place(id, parent_id, prev).await?;
            prev = Some(id);
        }
        Ok(())
    }

    /// Block id immediately preceding `id` among its siblings (same
    /// parent, strictly-lower sort_key, maximal under that constraint).
    /// `None` when `id` is the first child or has no block parent.
    async fn prev_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>>;

    /// Block id immediately following `id` among its siblings (same
    /// parent, strictly-higher sort_key, minimal under that constraint).
    /// `None` when `id` is the last child or has no block parent.
    async fn next_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>>;

    /// Block id of the first child of `parent_id` (lowest sort_key).
    /// `None` when `parent_id` has no children.
    async fn first_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>>;

    /// Block id of the last child of `parent_id` (highest sort_key).
    /// `None` when `parent_id` has no children.
    async fn last_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>>;

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
        _: &[EntityUri],
        _: &[EntityUri],
    ) -> Result<bool> {
        Ok(false)
    }

    /// [`create_in_tree`](Self::create_in_tree) for a CHUNK of creates, in
    /// request (document) order — one flag per request, same meaning.
    ///
    /// The org ingest calls this instead of one create per block because a
    /// tree-backed authority can then answer "does this id exist yet" once for
    /// the chunk and commit it once, turning per-block O(nodes) existence
    /// walks + per-block commits into one of each. Default: the per-block
    /// seam, so an implementation without a batched authority behaves
    /// identically (positional anchor `None`, exactly as the ingest passes it).
    async fn create_in_tree_batch(&self, requests: &[BlockCreateRequest]) -> Result<Vec<bool>> {
        let mut out = Vec::with_capacity(requests.len());
        for r in requests {
            out.push(
                self.create_in_tree(
                    &r.parent_id,
                    None,
                    &r.id,
                    r.content.clone(),
                    &r.properties,
                    &r.tags,
                    &r.requires,
                    &r.advice_suppressed,
                )
                .await?,
            );
        }
        Ok(out)
    }

    /// Whether `id` has a node in the separate authoritative tree.
    /// `Ok(None)` (default) when there is no separate tree — SqlOnly mode,
    /// or a backing where the store IS the tree — so the question doesn't
    /// apply. `Ok(Some(false))` is the pre-Loro-vault upgrade signal: the
    /// block exists in the SQL store but the Loro tree never adopted it
    /// (no seed pass ran when `[loro] enabled` flipped on). The org-scan
    /// reconciler uses this to re-seed such blocks via
    /// [`create_in_tree`](Self::create_in_tree).
    async fn in_tree(&self, _: &EntityUri) -> Result<Option<bool>> {
        Ok(None)
    }

    /// True when a separate upstream consolidator owns block writes — i.e. the
    /// outbound projector (`LoroSyncController::on_loro_changed`) is the sole
    /// writer of the SQL `block_raw` row. False in the direct-store mode, where
    /// the org reconciler must create rows directly via the command bus. Org
    /// ingestion uses this to skip the redundant command-bus block *create*
    /// when projected: the block lands upstream via `create_in_tree` and the
    /// projector writes the row, eliminating the dual-writer race on
    /// `sort_key`/`properties`. (Loro is the only upstream consolidator today.)
    fn has_upstream_consolidator(&self) -> bool {
        false
    }

    /// Which consolidator owns sibling order under the active session
    /// capability profile (`docs/Architecture/Replication.md` §2). This is the
    /// capability-typed successor to the raw
    /// [`has_upstream_consolidator`](Self::has_upstream_consolidator) boolean:
    /// order-decision call sites should branch on this, not on the bool, so
    /// widening the capability detection (D slice 2 — the full axes table) is a
    /// one-place change.
    fn consolidator(&self) -> Consolidator {
        if self.has_upstream_consolidator() {
            Consolidator::Upstream
        } else {
            Consolidator::Store
        }
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
    async fn update_in_tree(&self, params: holon_api::StorageEntity) -> Result<()>;

    /// Apply a block **delete** intent against the authoritative tree.
    ///
    /// `params` carries the `id` and the `ROUTING_DOC_URI_KEY` hint (so the
    /// SQL `prepare_delete` can skip the recursive document walk). Loro-backed
    /// impls delete from Loro (the outbound projector emits the SQL delete);
    /// SqlOnly impls delete the SQL row directly.
    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> Result<()>;

    /// Copy-on-write seed refresh: for each `(id, content)` whose block exists
    /// in the authority with DIFFERENT content, rewrite it to the current
    /// shipped-asset value (compares first, so an unchanged block emits no op).
    /// Used by `seed_default_layout` to auto-update the VIRTUAL default layout
    /// (`block:__default__`) from the bundled asset while the persisted Loro
    /// snapshot would otherwise pin a stale copy. Returns how many were
    /// refreshed. Default: `Ok(0)` — no separate authority to reconcile
    /// (SqlOnly / test stubs).
    async fn reseed_content(&self, _blocks: &[(EntityUri, String)]) -> Result<usize> {
        Ok(0)
    }

    /// Apply a whole file's worth of ordered ingest ops in one shot.
    ///
    /// `ops` is the org reconciler's `(op, params)` vector in **document
    /// order**: `create`/`update` (both routed through the update seam) then
    /// `delete`. The default implementation applies them one at a time via
    /// [`update_in_tree`](Self::update_in_tree) /
    /// [`delete_in_tree`](Self::delete_in_tree) — byte-identical to the
    /// historic boot loop, so Loro-backed and test impls need no override.
    ///
    /// The SqlOnly store overrides this to collapse the whole file's writes
    /// into ONE `db_handle.transaction()` so the live-watch matview IVM
    /// maintenance runs once per file instead of once per block. That is
    /// the fix for the O(N²) cold-boot ingest (BugFunnel row 32):
    /// per-single-row-write matview maintenance whose cost scales with the
    /// accumulated block table.
    async fn apply_ingest_batch(&self, ops: Vec<(String, holon_api::StorageEntity)>) -> Result<()> {
        for (op, params) in ops {
            match op.as_str() {
                "create" | "update" => self.update_in_tree(params).await?,
                "delete" => self.delete_in_tree(params).await?,
                other => {
                    return Err(format!("apply_ingest_batch: unknown op {other:?}").into());
                }
            }
        }
        Ok(())
    }

    /// All children of `parent_id` in positional order (low → high
    /// sort_key in SqlOnly mode; Loro tree order in Loro mode).
    /// Returns an empty Vec when there are no children.
    ///
    /// This is the authoritative ordering the system actually renders /
    /// projects from. Use it as the live-side ground truth in
    /// assertions instead of computing order from a `Block`'s
    /// `sort_key` / `sequence` field — those are encoding-specific.
    async fn children(&self, parent_id: &EntityUri) -> Result<Vec<EntityUri>>;
}

/// Order-key minting — the store-owner's exclusive right to synthesize a
/// fractional index (`sort_key`) for a NEW block.
///
/// This is deliberately a **separate** trait from [`BlockOrdering`], not a
/// method on it. Replication.md §5 requires that the mode which does NOT own
/// order be UNABLE to mint keys *by construction*. Only the SqlOnly / `Store`
/// consolidator owns sibling order, so only its ordering seam
/// (`SqlBlockOperations`) implements this trait. The Loro-mode seam
/// (`LoroBlockOrdering`) implements [`BlockOrdering`] alone and therefore has
/// no `-> String` key-minting method at all — in Loro mode the fractional
/// index is authoritative (Loro's tree owns it) and `apply_create` derives it
/// from `Event::position_after_block_id`, so a key never needs minting on that
/// path. The former placeholder `new_child_anchor` that returned a
/// `default_sort_key()` in Loro mode is thus unrepresentable, not just unused.
#[async_trait]
pub trait OrderKeyMinting: Send + Sync {
    /// Compute the `sort_key` value for a NEW block being created under
    /// `parent_id`, immediately after `after_id`, to persist verbatim in
    /// `block.sort_key`. Implemented only by the `Store` consolidator's order
    /// owner (the sole minter of fractional indices for its sibling sets).
    // Defining-module trait declaration (excluded at repo root; the exclude
    // glob misses the `.claude/worktrees/...` prefix, so annotate inline).
    // ALLOW(order_minting): trait-method declaration, not a mint call site.
    async fn new_child_anchor(
        &self,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> Result<String>;
}

#[cfg(test)]
mod default_contract_tests {
    use std::sync::Mutex;

    use super::*;

    /// Minimal ordering impl: records `place` calls, everything else is
    /// irrelevant to the default-method contracts under test.
    struct RecordingOrdering {
        places: Mutex<Vec<(String, String, Option<String>)>>,
    }

    #[async_trait]
    impl BlockOrdering for RecordingOrdering {
        async fn place(
            &self,
            uri: &EntityUri,
            parent_id: &EntityUri,
            after_id: Option<&EntityUri>,
        ) -> Result<()> {
            self.places.lock().unwrap().push((
                uri.as_str().to_string(),
                parent_id.as_str().to_string(),
                after_id.map(|u| u.as_str().to_string()),
            ));
            Ok(())
        }

        async fn prev_sibling(&self, _: &EntityUri) -> Result<Option<EntityUri>> {
            unimplemented!("not exercised")
        }
        async fn next_sibling(&self, _: &EntityUri) -> Result<Option<EntityUri>> {
            unimplemented!("not exercised")
        }
        async fn first_child(&self, _: &EntityUri) -> Result<Option<EntityUri>> {
            unimplemented!("not exercised")
        }
        async fn last_child(&self, _: &EntityUri) -> Result<Option<EntityUri>> {
            unimplemented!("not exercised")
        }
        async fn children(&self, _: &EntityUri) -> Result<Vec<EntityUri>> {
            unimplemented!("not exercised")
        }
        async fn update_in_tree(&self, _: holon_api::StorageEntity) -> Result<()> {
            unimplemented!("not exercised")
        }
        async fn delete_in_tree(&self, _: holon_api::StorageEntity) -> Result<()> {
            unimplemented!("not exercised")
        }
    }

    #[tokio::test]
    async fn default_place_all_places_each_id_after_its_predecessor() {
        let ordering = RecordingOrdering {
            places: Mutex::new(Vec::new()),
        };
        let parent = EntityUri::block("P");
        let ids = vec![
            EntityUri::block("A"),
            EntityUri::block("B"),
            EntityUri::block("C"),
        ];

        ordering.place_all(&parent, &ids).await.unwrap();

        let places = ordering.places.lock().unwrap();
        assert_eq!(
            *places,
            vec![
                ("block:A".to_string(), "block:P".to_string(), None),
                (
                    "block:B".to_string(),
                    "block:P".to_string(),
                    Some("block:A".to_string())
                ),
                (
                    "block:C".to_string(),
                    "block:P".to_string(),
                    Some("block:B".to_string())
                ),
            ],
            "place_all must realize the total order via predecessor-threaded place calls"
        );
    }

    #[tokio::test]
    async fn default_mode_signals_are_sql_only() {
        // SqlOnly-mode contracts: no separate tree (`in_tree` = None, the
        // question doesn't apply), no upstream consolidator, and therefore
        // the Store consolidator owns order. Flipping any of these silently
        // changes write routing (org-scan re-seed, command-bus create skip).
        let ordering = RecordingOrdering {
            places: Mutex::new(Vec::new()),
        };
        assert_eq!(
            ordering.in_tree(&EntityUri::block("A")).await.unwrap(),
            None
        );
        assert!(
            !ordering
                .create_in_tree(
                    &EntityUri::block("P"),
                    None,
                    &EntityUri::block("A"),
                    BlockContent::text("x"),
                    &std::collections::HashMap::new(),
                    &Tags::default(),
                    &[],
                    &[],
                )
                .await
                .unwrap(),
            "default create_in_tree must signal not-handled (SQL create path)"
        );
        assert!(!ordering.has_upstream_consolidator());
        assert!(matches!(
            ordering.consolidator(),
            holon_api::capability::Consolidator::Store
        ));
    }
}

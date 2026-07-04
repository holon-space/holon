//! `SqlBlockOperations` — registers `BlockOperations` (`indent`, `outdent`,
//! `move_block`, `move_up`, `move_down`, `split_block`) as a separate
//! `OperationProvider` for the `"block"` entity.
//!
//! `SqlOperationProvider` advertises only the generic CRUD ops
//! (`set_field` / `create` / `update` / `delete` / `cycle_task_state`).
//! Without this provider, nothing in the dispatcher answers an `indent`
//! request — the keychord registered for Tab in
//! `holon-frontend/src/reactive.rs` cannot bind to any widget, and
//! `bubble_input` returns `false` ("Keychord did not match"). See the
//! comment on `BLOCK_TREE_KEYCHORD_OPS_ENABLED` in
//! `crates/holon-integration-tests/src/pbt/state_machine.rs` for the full
//! diagnosis of the production gap.
//!
//! This provider runs the trait default implementations from
//! `BlockOperations`, which decompose into a sequence of `set_field` calls.
//! In Loro mode block writes route through the `BlockCellRegistry` to Loro (the
//! authority) and the outbound projector emits the SQL row; in SqlOnly mode
//! they land in SQL directly via `SqlOperationProvider::execute_operation`.
//! There is no SQL→Loro reflection. Reads come from `QueryableCache<Block>` —
//! same backing store as the rest of the system.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OperationDescriptor;
use holon_api::Tags;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::capability::Consolidator;
use holon_api::capability::SessionCapabilities;
use holon_core::BlockDataSourceHelpers;
use holon_core::BlockMaintenanceHelpers;
use holon_core::BlockOperations;
use holon_core::BlockQueryHelpers;
use holon_core::CrudOperations;
use holon_core::DataSource;
use holon_core::OperationProvider;
use holon_core::OperationRegistry;
use holon_core::OperationResult;
use holon_core::OriginTaggedWrites;
use holon_core::Result;
use holon_core::UnknownOperationError;
use holon_core::block_ordering::BlockOrdering;
use holon_core::block_ordering::OrderKeyMinting;
use holon_core::cell_registry::EntityCellRegistry;
use holon_core::fractional_index::default_sort_key;
use holon_core::fractional_index::gen_key_between;
use holon_core::fractional_index::gen_n_keys;
use holon_core::storage::types::StorageEntity;

use crate::core::queryable_cache::HasCache;
use crate::core::queryable_cache::QueryableCache;
use crate::core::sql_operation_provider::SqlOperationProvider;
use crate::sync::block_cell_registry::BlockCellRegistry;
use crate::sync::event_bus::EventOrigin;

pub struct SqlBlockOperations {
    sql_ops: Arc<SqlOperationProvider>,
    cache: Arc<QueryableCache<Block>>,
    /// Cell registry for block fields. Populated from DI at construction
    /// (`event_infra_module.rs`); chord-time ops route content reads/
    /// writes through this so the live Loro view is consulted before the
    /// lagging SQL `block.content` projection. `None`-equivalent
    /// behaviour for synthetic `MemStore` test impls is achieved by them
    /// inheriting the `BlockOperations::cells()` default of `None`
    /// rather than calling into this struct.
    cell_registry: Arc<BlockCellRegistry>,
    /// The session's order/merge role, injected by the DI composition root —
    /// the SQL component is *told* whether it owns order, it does not probe a
    /// Loro-aware component to find out. This is the dependency inversion that
    /// keeps `SqlBlockOperations` ignorant of Loro: it acts on its capability
    /// (`Consolidator::Store` ⇒ mint keys / write directly; otherwise an
    /// upstream consolidator owns order and this component defers). Defaults to
    /// the direct-store profile so non-DI/test construction degrades safely.
    caps: SessionCapabilities,
}

impl SqlBlockOperations {
    /// Construct without a cell registry. Defaults to a `sql_only()`
    /// registry so chord ops degrade to direct SQL writes when no
    /// LoroModule is loaded. Production callers pair this with
    /// `with_cell_registry` in their DI factory closure.
    pub fn new(sql_ops: Arc<SqlOperationProvider>, cache: Arc<QueryableCache<Block>>) -> Self {
        Self {
            sql_ops,
            cache,
            cell_registry: Arc::new(BlockCellRegistry::sql_only()),
            caps: SessionCapabilities::detect_and_pin(false),
        }
    }

    /// Attach a cell registry resolved from DI. Used by the
    /// `event_infra_module` factory so chord-time ops route content
    /// reads/writes through `BlockCellRegistry::live_field<String>` (and
    /// hence the live Loro `LoroText` view) instead of the SQL cache.
    pub fn with_cell_registry(mut self, registry: Arc<BlockCellRegistry>) -> Self {
        self.cell_registry = registry;
        self
    }

    /// Pin the session's capability role (who owns order/merge). The DI
    /// composition root resolves this once from what is actually present and
    /// hands it in; this component never asks "is Loro present" itself.
    pub fn with_capabilities(mut self, caps: SessionCapabilities) -> Self {
        self.caps = caps;
        self
    }

    /// Ordered `(id, sort_key)` pairs for a parent, read straight from the
    /// internal `sort_key` column via the cache wrapper and sorted on it
    /// (`(sort_key, id)` lexicographic — Turso IVM matviews can't `ORDER BY`,
    /// so the wrapper sorts). Used by the SqlOnly key-minting writer; the
    /// `sort_key` encoding stays internal and never surfaces on the domain
    /// `Block` (ADR 0005).
    async fn sibling_keys(&self, parent_id: &str) -> Result<Vec<(String, String)>> {
        let rows = self
            .cache
            .query_raw("parent_id = ?", vec![Value::String(parent_id.to_string())])
            .await?;
        let mut pairs: Vec<(String, String)> = rows
            .into_iter()
            .filter_map(|r| {
                let id = r
                    .get("id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)?;
                let sk = r
                    .get("sort_key")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)?;
                Some((id, sk))
            })
            .collect();
        pairs.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        Ok(pairs)
    }
}

#[async_trait]
impl DataSource<Block> for SqlBlockOperations {
    async fn get_all(&self) -> Result<Vec<Block>> {
        self.cache.get_all().await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Block>> {
        self.cache.get_by_id(id).await
    }
}

impl HasCache<Block> for SqlBlockOperations {
    fn get_cache(&self) -> &QueryableCache<Block> {
        &self.cache
    }
}

#[async_trait]
impl BlockQueryHelpers<Block> for SqlBlockOperations {
    // Route the sibling/child lookups through `BlockOrdering` instead of
    // filtering blocks by `Block::sort_key()` in-memory. Same source
    // (the SQL cache) — the indirection just keeps `sort_key` reads
    // confined to the ordering adapter so a future backing without a
    // fractional-index string only has to change `BlockOrdering`.
    async fn get_prev_sibling(&self, block_id: &EntityUri) -> Result<Option<Block>> {
        match <Self as BlockOrdering>::prev_sibling(self, block_id).await? {
            Some(id) => self.get_by_id(id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn get_next_sibling(&self, block_id: &EntityUri) -> Result<Option<Block>> {
        match <Self as BlockOrdering>::next_sibling(self, block_id).await? {
            Some(id) => self.get_by_id(id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn get_first_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<Block>> {
        let Some(pid) = parent_id else {
            return Ok(None);
        };
        match <Self as BlockOrdering>::first_child(self, pid).await? {
            Some(id) => self.get_by_id(id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn get_last_child(&self, parent_id: Option<&EntityUri>) -> Result<Option<Block>> {
        let Some(pid) = parent_id else {
            return Ok(None);
        };
        match <Self as BlockOrdering>::last_child(self, pid).await? {
            Some(id) => self.get_by_id(id.as_str()).await,
            None => Ok(None),
        }
    }

    async fn children_ordered(&self, parent_id: &EntityUri) -> Result<Vec<Block>> {
        // The ordering authority is `BlockOrdering::children` (Loro live tree
        // in Loro mode, else the SQL cache ordered by the internal sort_key
        // column). Resolve ids there, then hydrate the domain blocks.
        let ids = <Self as BlockOrdering>::children(self, parent_id).await?;
        let mut out = Vec::with_capacity(ids.len());
        for id in ids {
            if let Some(b) = self.get_by_id(id.as_str()).await? {
                out.push(b);
            }
        }
        Ok(out)
    }
}
impl BlockMaintenanceHelpers<Block> for SqlBlockOperations {}
#[async_trait]
impl BlockDataSourceHelpers<Block> for SqlBlockOperations {
    /// Read the `Page` tag from the write authority (`block_tags`) instead of
    /// the `block`-matview-projected `Block::tags`, which trails the edge write
    /// via CDC. Closes the read-snapshot window that let a day-page's child
    /// escape into `journals` during tag-propagation lag (journals-phantom).
    async fn is_page_authoritative(&self, id: &holon_api::EntityUri) -> Result<bool> {
        self.sql_ops.block_is_page(id.as_str()).await
    }
}
impl BlockOperations<Block> for SqlBlockOperations {
    fn cells(&self) -> Option<&dyn EntityCellRegistry> {
        Some(&*self.cell_registry as &dyn EntityCellRegistry)
    }

    fn ordering(&self) -> Option<&dyn BlockOrdering> {
        Some(self as &dyn BlockOrdering)
    }

    fn order_key_minter(&self) -> Option<&dyn OrderKeyMinting> {
        Some(self as &dyn OrderKeyMinting)
    }
}

/// Pure monotonic relabel: given `ordered_ids` (the intended order) and the key
/// each currently carries (`cur_keys`, aligned by index; the default sentinel
/// `default_sort_key()` means "unkeyed"), return the key each id should
/// have so that lexical `sort_key` order reproduces `ordered_ids`. Keys already
/// strictly above their predecessor's final key are kept verbatim; only
/// violators (out-of-place, or carrying the default sentinel) are re-minted
/// strictly between their predecessor's final key and the next keepable anchor.
///
/// The default sentinel is never "keepable": it is not a `gen_key_between`
/// value and lands arbitrarily in the lexical keyspace (`"A0"` sorts *above*
/// real hex-ish indices like `"80"`), so an unkeyed block must always receive a
/// real minted key (projection totality). Idempotent: a fully-keyed,
/// already-ordered input returns its input unchanged.
fn relabel_order(ordered_ids: &[&str], cur_keys: &[String]) -> Result<Vec<String>> {
    let default_key = default_sort_key();
    let keepable =
        |k: &str, prev: Option<&str>| -> bool { k != default_key && prev.is_none_or(|p| k > p) };
    let mut out: Vec<String> = Vec::with_capacity(ordered_ids.len());
    let mut prev_final: Option<String> = None;
    for i in 0..ordered_ids.len() {
        if keepable(&cur_keys[i], prev_final.as_deref()) {
            prev_final = Some(cur_keys[i].clone());
            out.push(cur_keys[i].clone());
            continue;
        }
        let upper: Option<&str> = ((i + 1)..ordered_ids.len())
            .map(|j| cur_keys[j].as_str())
            .find(|kj| keepable(kj, prev_final.as_deref()));
        let new_key = gen_key_between(prev_final.as_deref(), upper)
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })?;
        prev_final = Some(new_key.clone());
        out.push(new_key);
    }
    Ok(out)
}

#[async_trait]
impl OrderKeyMinting for SqlBlockOperations {
    /// Returns the SQL `block.sort_key` value to persist for a new block
    /// placed under `parent_id` after `after_id`. Real minting happens only
    /// when this store is the `Store` consolidator (SqlOnly); in Upstream
    /// (Loro) mode the body short-circuits to `default_sort_key()` because the
    /// tree owns the fractional index — `apply_create` overwrites the value
    /// after `Event::position_after_block_id` drives `tree.mov_after`. That
    /// residual Upstream reachability exists because this hybrid store's
    /// `place()` falls through to minting for blocks not yet in the Loro tree
    /// (the disclosed SQL-path carve-out for blocks absent from the tree); the
    /// pure Loro ordering seam has no minting method at all (`OrderKeyMinting`,
    /// Replication.md §5).
    ///
    /// Tied-key rebalance: if any two siblings under `parent_id` already
    /// share a `sort_key` (e.g. parser-default `"A0"` after a bulk file
    /// write), `gen_key_between` can't produce a strictly-between key for
    /// the new slot. We detect the tie and redistribute all siblings into
    /// distinct evenly-spaced keys via `gen_n_keys`, with the new block's
    /// slot inserted at the correct position. Existing siblings are
    /// updated with a single `set_field("sort_key")` each (no re-reads
    /// between writes — values are computed up-front so the chord-op
    /// projection race documented in MEMORY can't bite). The new block's
    /// key is returned for the caller to persist on create.
    async fn new_child_anchor(
        &self,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> Result<String> {
        let parent_id = parent_id.as_str();
        let after_id: Option<&str> = after_id.map(|u| u.as_str());
        // P1 isolation: only the SqlOnly (no-Loro) order owner mints keys here.
        // In Loro mode the fractional index is authoritative — `apply_create`
        // sets it from `position_after_block_id` and this return value is
        // discarded — so short-circuit BEFORE the `gen_key_between` generator
        // and its tied-key rebalance, which would otherwise emit spurious
        // sibling `set_field("sort_key")` writes against the Loro-projected SQL
        // view. The placeholder routes through `default_sort_key()` (the single
        // default owner), never a stray literal.
        if matches!(self.consolidator(), Consolidator::Upstream) {
            return Ok(default_sort_key());
        }
        // `(id, sort_key)` pairs already in `(sort_key, id)` order — matches
        // `OrgRenderer::render_entity_tree` so the new block's "after" slot is
        // interpreted in the same order the on-disk render uses.
        let siblings = self.sibling_keys(parent_id).await?;

        let has_ties = siblings.windows(2).any(|w| w[0].1 == w[1].1);

        if has_ties {
            // Insertion index: where the new block lands in the rebalanced
            // sequence. With no `after_id` it's slot 0 (first child).
            let insert_idx = match after_id {
                None => 0usize,
                Some(after) => {
                    siblings.iter().position(|(id, _)| id == after).ok_or_else(
                        || -> Box<dyn std::error::Error + Send + Sync> {
                            format!("new_child_anchor: after block {after} missing").into()
                        },
                    )? + 1
                }
            };
            let new_keys = gen_n_keys(siblings.len() + 1).map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() },
            )?;
            let new_block_key = new_keys[insert_idx].clone();
            let entity = EntityName::new(Block::entity_name());
            for (i, (sib_id, sib_key)) in siblings.iter().enumerate() {
                let target_key = if i < insert_idx {
                    &new_keys[i]
                } else {
                    &new_keys[i + 1]
                };
                if sib_key == target_key {
                    continue;
                }
                let mut params: StorageEntity = HashMap::new();
                params.insert("id".into(), Value::String(sib_id.clone()));
                params.insert("field".into(), Value::String("sort_key".to_string()));
                params.insert("value".into(), Value::String(target_key.clone()));
                self.sql_ops
                    .execute_operation(&entity, "set_field", params)
                    .await?;
            }
            return Ok(new_block_key);
        }

        let (prev_key, next_key): (Option<String>, Option<String>) = match after_id {
            None => {
                let first = siblings.first().map(|(_, sk)| sk.clone());
                (None, first)
            }
            Some(after) => {
                let after_key = siblings
                    .iter()
                    .find(|(id, _)| id == after)
                    .map(|(_, sk)| sk.clone())
                    .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("new_child_anchor: after block {after} missing").into()
                    })?;
                let next = siblings
                    .iter()
                    .find(|(_, sk)| *sk > after_key)
                    .map(|(_, sk)| sk.clone());
                (Some(after_key), next)
            }
        };
        gen_key_between(prev_key.as_deref(), next_key.as_deref())
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }
}

#[async_trait]
impl BlockOrdering for SqlBlockOperations {
    /// Loro mode → `write_position` (tree.mov_after). SqlOnly mode →
    /// `new_child_anchor` + paired `set_field("parent_id") +
    /// set_field("sort_key")` via the underlying `OperationProvider`.
    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> Result<()> {
        if self
            .cell_registry
            .write_position(uri, parent_id.as_str(), after_id.map(|u| u.as_str()))
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })?
        {
            return Ok(());
        }
        // SqlOnly idempotency guard: skip if current parent_id and predecessor
        // already match the requested placement. Without this, new_child_anchor's
        // neighbor scan includes the target itself as the next sibling, so
        // gen_key_between mints a fresh sort_key on every call — causing a loop.
        // Use uri.as_str() (full URI like "block:abc") for cache lookups — the
        // DB stores full URIs, not bare IDs.
        {
            let current_block_opt = self.cache.get_by_id(uri.as_str()).await.map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("place: cache get_by_id {}: {e:#}", uri.as_str()).into()
                },
            )?;
            if let Some(current_block) = current_block_opt {
                let current_prev = self.prev_sibling(uri).await.map_err(
                    |e| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("place: prev_sibling {}: {e:#}", uri.as_str()).into()
                    },
                )?;
                if &current_block.parent_id == parent_id && current_prev.as_ref() == after_id {
                    return Ok(());
                }
            }
        }
        let new_sort_key = self.new_child_anchor(parent_id, after_id).await?;
        // SQL stores full URI form ("block:..."); set_field's UPDATE WHERE id = ?
        // does a literal string match. Passing the bare id silently matches
        // zero rows (the test `place_is_idempotent_in_sql_only_mode` doesn't
        // catch this — its idempotency guard short-circuits before the write).
        let id = uri.as_str();
        let mut parent_params: StorageEntity = HashMap::new();
        parent_params.insert("id".into(), Value::String(id.to_string()));
        parent_params.insert("field".into(), Value::String("parent_id".to_string()));
        parent_params.insert(
            "value".into(),
            Value::String(parent_id.as_str().to_string()),
        );
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation(&entity, "set_field", parent_params)
            .await?;
        let mut sort_params: StorageEntity = HashMap::new();
        sort_params.insert("id".into(), Value::String(id.to_string()));
        sort_params.insert("field".into(), Value::String("sort_key".to_string()));
        sort_params.insert("value".into(), Value::String(new_sort_key));
        self.sql_ops
            .execute_operation(&entity, "set_field", sort_params)
            .await?;
        Ok(())
    }

    /// Total, minimal-diff re-key for the SQL (no-Loro) order owner.
    ///
    /// The org re-ingest hands us the file's complete line order for a parent;
    /// SQL is the sole order owner in this configuration, so we must make
    /// `ORDER BY sort_key` reproduce `ordered_ids` exactly. The incremental
    /// `place` loop can't converge a full reorder, but a naïve full rewrite
    /// (`gen_n_keys` for every child on every change) is O(N) key churn — the
    /// thing `Replication.md` §5 warns against — and floods CDC.
    ///
    /// So we do a **monotonic relabel**: walk `ordered_ids` left→right keeping
    /// the running maximum assigned key; a block whose current key is already
    /// strictly greater than its predecessor's final key is left untouched, and
    /// only an out-of-place block is re-minted with `gen_key_between` strictly
    /// between its predecessor's final key and the next key we will keep.
    /// Result is a strictly-increasing total order (correct) that touches
    /// only the blocks that actually moved (low churn). Idempotent: an
    /// already-ordered parent makes zero writes.
    async fn place_all(&self, parent_id: &EntityUri, ordered_ids: &[EntityUri]) -> Result<()> {
        if ordered_ids.is_empty() {
            return Ok(());
        }
        let parent_str = parent_id.as_str();
        // (parent_id, sort_key) per block, read from the raw rows — the
        // internal `sort_key` column is no longer on the domain `Block`.
        let rows = self.cache.query_raw("", vec![]).await?;
        let by_id: HashMap<String, (String, String)> = rows
            .into_iter()
            .filter_map(|r| {
                let id = r
                    .get("id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)?;
                let parent = r
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
                    .unwrap_or_default();
                let sk = r
                    .get("sort_key")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
                    .unwrap_or_else(default_sort_key);
                Some((id, (parent, sk)))
            })
            .collect();
        let entity = EntityName::new(Block::entity_name());
        let default_key = default_sort_key();

        // Resolve current keys in target order, fixing parent_id where it
        // diverges (rare: a re-parent into this parent). A missing block is
        // treated as unkeyed (default sentinel) so a not-yet-projected row
        // still gets a real key rather than panicking the scan.
        let mut cur_keys: Vec<String> = Vec::with_capacity(ordered_ids.len());
        for id in ordered_ids {
            match by_id.get(id.as_str()) {
                Some((parent, sk)) => {
                    if parent != parent_str {
                        let mut parent_params: StorageEntity = HashMap::new();
                        parent_params.insert("id".into(), Value::String(id.as_str().to_string()));
                        parent_params
                            .insert("field".into(), Value::String("parent_id".to_string()));
                        parent_params.insert("value".into(), Value::String(parent_str.to_string()));
                        self.sql_ops
                            .execute_operation(&entity, "set_field", parent_params)
                            .await?;
                    }
                    cur_keys.push(sk.clone());
                }
                None => cur_keys.push(default_key.clone()),
            }
        }

        // Compute the total order key for each child (keep already-ordered keys,
        // re-mint only violators) and write only the ones that changed.
        // Idempotent: a parent whose children carry born-correct keys makes
        // zero writes — so the common org-ingest path (blocks created already
        // carrying their order key, see `order_keys`) leaves no half-applied
        // window for a concurrent reader.
        let ordered_strs: Vec<&str> = ordered_ids.iter().map(|u| u.as_str()).collect();
        let target = relabel_order(&ordered_strs, &cur_keys)?;
        for (i, key) in target.iter().enumerate() {
            if *key != cur_keys[i] {
                let mut sort_params: StorageEntity = HashMap::new();
                sort_params.insert(
                    "id".into(),
                    Value::String(ordered_ids[i].as_str().to_string()),
                );
                sort_params.insert("field".into(), Value::String("sort_key".to_string()));
                sort_params.insert("value".into(), Value::String(key.clone()));
                self.sql_ops
                    .execute_operation(&entity, "set_field", sort_params)
                    .await?;
            }
        }
        Ok(())
    }

    /// Loro mode → `create_entity` (LoroBackend::create_block, synchronous).
    /// SqlOnly mode → `false`, so the caller keeps its SQL create path.
    async fn create_in_tree(
        &self,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
        new_id: &EntityUri,
        content: holon_api::BlockContent,
        properties: &HashMap<String, Value>,
        tags: &Tags,
        requires: &[EntityUri],
        advice_suppressed: &[EntityUri],
    ) -> Result<bool> {
        self.cell_registry
            .create_entity(
                parent_id,
                after_id,
                new_id,
                content,
                properties,
                tags,
                requires,
                advice_suppressed,
            )
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    /// One warm + one commit for the whole chunk (see
    /// `BlockCellRegistry::create_entities`) instead of the default's
    /// per-block create — the cold-boot ingest's dominant term.
    async fn create_in_tree_batch(
        &self,
        requests: &[holon_core::block_ordering::BlockCreateRequest],
    ) -> Result<Vec<bool>> {
        self.cell_registry
            .create_entities(requests)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    async fn in_tree(&self, id: &EntityUri) -> Result<Option<bool>> {
        self.cell_registry
            .live_in_tree(id.as_str())
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    /// Apply a block update intent — the single org→block mutation seam (no
    /// command bus behind it).
    ///
    /// Loro mode: route each changed field through
    /// [`set_field`](CrudOperations::set_field) (→ Loro via the cell
    /// registry; the outbound projector writes the SQL row) and the
    /// position through [`place`](Self::place). SqlOnly mode: write the SQL
    /// row directly, picking `create`/`update` by the row's prior
    /// presence so the emitted CDC event kind matches (the cache subscriber
    /// distinguishes `Created`/`Updated`). A block is either a create or an
    /// update within a single org scan, never both, so the cache read is a
    /// reliable presence test.
    async fn update_in_tree(&self, mut params: holon_api::StorageEntity) -> Result<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                "update_in_tree: missing 'id' param".into()
            })?;
        let after = params
            .remove(crate::sync::event_bus::POSITION_AFTER_BLOCK_ID_PARAM)
            .and_then(|v| v.as_string().map(str::to_string));

        if matches!(self.consolidator(), Consolidator::Upstream) {
            let parent_id = params
                .get("parent_id")
                .and_then(|v| v.as_string())
                .map(str::to_string);
            // Content / edge / scalar fields → Loro via set_field. Skip the
            // primary key, the routing hint (not a field), and parent_id +
            // position which `place` owns.
            for (field, value) in params.into_iter() {
                if &*field == "id"
                    || &*field == "parent_id"
                    || &*field == crate::sync::event_bus::ROUTING_DOC_URI_KEY
                {
                    continue;
                }
                self.set_field(&id, &field, value).await?;
            }
            // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
            let block_uri = EntityUri::from_raw(&id);
            match (parent_id, after) {
                (Some(parent), Some(after_id)) => {
                    // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
                    let parent_uri = EntityUri::from_raw(&parent);
                    // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
                    let after_uri = EntityUri::from_raw(&after_id);
                    self.place(&block_uri, &parent_uri, Some(&after_uri))
                        .await?;
                }
                // No recorded predecessor: set the parent; the org reconciler's
                // disk-order replay loop finalises first-child ordering.
                (Some(parent), None) => {
                    self.set_field(&id, "parent_id", Value::String(parent))
                        .await?;
                }
                (None, _) => {}
            }
        } else {
            let op = if self.cache.get_by_id(&id).await?.is_some() {
                "update"
            } else {
                "create"
            };
            // SQL is the order owner: a freshly created block is born carrying
            // its real order key — minted between its file predecessor and the
            // next sibling — so the create and its keying are a single write and
            // the row is never observable at the default `"A0"`. This closes the
            // create-default-then-rekey window that let a concurrent reader (or
            // the PBT invariant) see a half-applied sibling order. Updates keep
            // their key; position changes route through `place`.
            if op == "create"
                && let Some(parent) = params
                    .get("parent_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string)
            {
                // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
                let parent_uri = EntityUri::from_raw(&parent);
                let after_uri = after.as_deref().map(EntityUri::from_raw);
                let key = self
                    .new_child_anchor(&parent_uri, after_uri.as_ref())
                    .await?;
                params.insert("sort_key".into(), Value::String(key));
            }
            if let Some(after_id) = after {
                params.insert(
                    crate::sync::event_bus::POSITION_AFTER_BLOCK_ID_PARAM.into(),
                    Value::String(after_id),
                );
            }
            let entity = EntityName::new(Block::entity_name());
            self.sql_ops
                .execute_operation_with_origin(&entity, op, params, EventOrigin::Org)
                .await?;
        }
        Ok(())
    }

    /// Apply a block delete intent.
    ///
    /// Loro mode: delete from the Loro tree (the authority) via the cell
    /// registry; the outbound projector emits the SQL DELETE. This mirrors
    /// `create_in_tree` (creates go to Loro, the projector writes SQL).
    /// Deleting only from SQL would race the armed projection, which
    /// re-creates the still-present Loro node back into SQL — the block
    /// resurrects (observed as `inv-backend-blocks-match-ref` spurious
    /// `bulk-*` rows).
    ///
    /// SqlOnly mode: the registry returns `false`; delete straight from SQL via
    /// the operation provider, preserving the `ROUTING_DOC_URI_KEY` hint so
    /// `prepare_delete` skips the recursive document walk.
    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> Result<()> {
        let id = params.get("id").and_then(|v| v.as_string()).ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                "delete_in_tree: missing 'id' param".into()
            },
        )?;
        // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation params dict
        let uri = EntityUri::from_raw(id);
        if self
            .cell_registry
            .delete_entity(&uri)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })?
        {
            // Loro-backed: the outbound projector writes the SQL DELETE.
            return Ok(());
        }
        // ALLOW(fallback): authority-first delete guard (Task 3) — the SQL delete
        // path here is only reachable for unseeded blocks and SqlOnly mode. In Loro
        // mode, log a warning so we observe how often the transitional path fires.
        if self.cell_registry.has_loro_backing() {
            tracing::warn!(
                block_id = %id, // ALLOW(fallback): disclosed degraded-mode warning, transitional path
                "SQL delete fallback in Loro mode — block was unseeded. \
                 Transitional; re-seed adoption eliminates unseeded blocks."
            );
        }
        // ALLOW(sole_block_writer) ALLOW(fallback): SQL delete fallback for unseeded
        // blocks. Transitional — after sole-writer, all blocks originate in
        // Loro, so this fires only for pre-existing unseeded vaults the re-seed
        // adoption pass eliminates. NOT dead code: re-seed can fail mid-adoption.
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation_with_origin(&entity, "delete", params, EventOrigin::Org)
            .await?;
        Ok(())
    }

    async fn reseed_content(&self, blocks: &[(EntityUri, String)]) -> Result<usize> {
        self.cell_registry
            .reseed_content(blocks)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    /// Apply a whole file's ingest ops in ONE transaction (BugFunnel row 32).
    ///
    /// The historic boot loop applied each block through
    /// [`update_in_tree`](Self::update_in_tree) / `delete_in_tree`, so every
    /// single-row write drove a matview IVM maintenance pass over the live
    /// watch-views — cost scaling with the accumulated block table (O(N²) cold
    /// boot). SqlOnly is the SQL store's own keyspace, so the whole file's ops
    /// route through `SqlOperationProvider::execute_batch_with_origin` — ONE
    /// `db_handle.transaction()`, hence ONE maintenance pass per file.
    ///
    /// Creates are born carrying a strictly-increasing per-parent
    /// document-order `sort_key`, minted in-memory (seeded once per parent from
    /// the current sibling set — empty on cold boot). That matches what the
    /// downstream SqlOnly `place_all` totalizer expects, so it finds every new
    /// child already ordered and issues ZERO single-row `set_field("sort_key")`
    /// rewrites — otherwise the per-block cost merely migrates into
    /// `place_all`.
    ///
    /// In Upstream (Loro) mode field writes route through the cell registry
    /// (Loro owns order), NOT this SQL batch sink, so the per-op default is
    /// kept verbatim — the O(N²) this fixes was measured SqlOnly only.
    async fn apply_ingest_batch(&self, ops: Vec<(String, StorageEntity)>) -> Result<()> {
        if matches!(self.consolidator(), Consolidator::Upstream) {
            for (op, params) in ops {
                match op.as_str() {
                    "create" | "update" => self.update_in_tree(params).await?,
                    "delete" => self.delete_in_tree(params).await?,
                    other => {
                        return Err(format!("apply_ingest_batch: unknown op {other:?}").into());
                    }
                }
            }
            return Ok(());
        }

        let entity = EntityName::new(Block::entity_name());
        let mut batch: Vec<(String, StorageEntity)> = Vec::with_capacity(ops.len());
        // Per-parent last-assigned sort_key cursor, seeded lazily from the DB
        // sibling set the first time a parent is touched.
        let mut parent_cursor: HashMap<String, Option<String>> = HashMap::new();
        for (op, mut params) in ops {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .map(str::to_string)
                .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                    "apply_ingest_batch: op missing 'id' param".into()
                })?;
            match op.as_str() {
                "create" | "update" => {
                    // Re-derive create vs update from the SQL cache — the same
                    // classification `update_in_tree` makes, so the CDC op kind
                    // matches the row's prior presence.
                    let real_op = if self.cache.get_by_id(&id).await?.is_some() {
                        "update"
                    } else {
                        "create"
                    };
                    if real_op == "create"
                        && let Some(parent) = params
                            .get("parent_id")
                            .and_then(|v| v.as_string())
                            .map(str::to_string)
                    {
                        let cursor = match parent_cursor.entry(parent.clone()) {
                            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
                            std::collections::hash_map::Entry::Vacant(e) => {
                                let existing = self.sibling_keys(&parent).await?;
                                e.insert(existing.last().map(|(_, k)| k.clone()))
                            }
                        };
                        // SqlOnly `Store` order owner — the sanctioned mint site
                        // (Replication.md §5): batched form of `new_child_anchor`'s
                        // append path with an in-memory per-parent cursor.
                        // ALLOW(order_minting): SqlOnly order owner, batched mint.
                        let key = gen_key_between(cursor.as_deref(), None).map_err(
                            |e| -> Box<dyn std::error::Error + Send + Sync> {
                                format!("apply_ingest_batch mint for {id}: {e:#}").into()
                            },
                        )?;
                        *cursor = Some(key.clone());
                        params.insert("sort_key".into(), Value::String(key));
                    }
                    batch.push((real_op.to_string(), params));
                }
                "delete" => batch.push(("delete".to_string(), params)),
                other => {
                    return Err(format!("apply_ingest_batch: unknown op {other:?}").into());
                }
            }
        }
        self.sql_ops
            .execute_batch_with_origin(&entity, batch, EventOrigin::Org)
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("apply_ingest_batch execute_batch_with_origin: {e:#}").into()
            })?;
        Ok(())
    }

    fn has_upstream_consolidator(&self) -> bool {
        self.caps.profile().has_downstream_projection()
    }

    fn consolidator(&self) -> Consolidator {
        self.caps.consolidator()
    }

    async fn prev_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        let id = id.as_str();
        let Some(block) = self.cache.get_by_id(id).await? else {
            return Err(format!("prev_sibling: block {id} missing").into());
        };
        if !block.parent_id.is_block() {
            return Ok(None);
        }
        let siblings = self.sibling_keys(block.parent_id.as_str()).await?;
        let pos = siblings.iter().position(|(sid, _)| sid == id);
        Ok(pos
            .and_then(|i| i.checked_sub(1))
            .and_then(|i| siblings.get(i))
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(sid, _)| EntityUri::from_raw(sid)))
    }

    async fn next_sibling(&self, id: &EntityUri) -> Result<Option<EntityUri>> {
        let id = id.as_str();
        let Some(block) = self.cache.get_by_id(id).await? else {
            return Err(format!("next_sibling: block {id} missing").into());
        };
        if !block.parent_id.is_block() {
            return Ok(None);
        }
        let siblings = self.sibling_keys(block.parent_id.as_str()).await?;
        let pos = siblings.iter().position(|(sid, _)| sid == id);
        Ok(pos
            .and_then(|i| siblings.get(i + 1))
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(sid, _)| EntityUri::from_raw(sid)))
    }

    async fn first_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(self
            .sibling_keys(parent_id.as_str())
            .await?
            .into_iter()
            .next()
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(id, _)| EntityUri::from_raw(&id)))
    }

    async fn last_child(&self, parent_id: &EntityUri) -> Result<Option<EntityUri>> {
        Ok(self
            .sibling_keys(parent_id.as_str())
            .await?
            .into_iter()
            .last()
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(id, _)| EntityUri::from_raw(&id)))
    }

    async fn children(&self, parent_id: &EntityUri) -> Result<Vec<EntityUri>> {
        let parent_id = parent_id.as_str();
        // Loro mode: the Loro tree is the order authority. Read it directly so
        // the org-scan place loop sees `create_in_tree` blocks immediately —
        // the outbound projector that fills the SQL cache is not running during
        // the initial scan, so a cache read returns `[]` for freshly-created
        // blocks and the poll times out. Steady-state, the cache is just a
        // projection of this same tree, so the answer is identical.
        if let Some(kids) = self.cell_registry.live_children(parent_id).await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("children({parent_id}): {e:#}").into()
            },
        )? {
            // ALLOW(entity_uri_from_raw): child id String from
            // cell_registry.live_children() (Loro output)
            return Ok(kids.iter().map(|k| EntityUri::from_raw(k)).collect());
        }
        Ok(self
            .sibling_keys(parent_id)
            .await?
            .into_iter()
            // ALLOW(entity_uri_from_raw): sibling/child id String from sibling_keys() SQL-cache
            // rows
            .map(|(id, _)| EntityUri::from_raw(&id))
            .collect())
    }
}

#[async_trait]
impl CrudOperations<Block> for SqlBlockOperations {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
        // Phase 2 authority flip: route block field writes through the
        // `BlockCellRegistry`, which writes to Loro (LoroText for `content`,
        // `tree.mov` for `parent_id`, LoroMap meta for the rest). The
        // `LoroSyncController.on_loro_changed` outbound projector then
        // emits the SQL UPDATE — there's no SQL write on this path. The
        // registry returns `Ok(false)` for fields it can't handle (SqlOnly
        // mode, or fields like `sort_key`/`marks`/`depth` whose Loro
        // encoding doesn't round-trip cleanly today); on `Ok(false)` we
        // fall through to the legacy SQL `set_field` path so existing
        // behaviour is preserved for those fields.
        // `id` arrives in mixed forms: already-schemed (`block:foo`) from the
        // org update path (`build_block_params` → `block.id.to_string()`), or
        // bare (`foo`) from some `LoroBlockOperations` callers. `from_raw`
        // normalizes both (parses a schemed id as-is, treats a bare id as a
        // block) — `EntityUri::block(id)` double-prefixed a schemed id to
        // `block:block:foo`, which `write_field`/`update_block_fields` then
        // failed to resolve ("Block not found"), aborting the org scan's update
        // pass *before* the place loop ran (the BulkExternalAdd sibling-order
        // scramble, `inv-live-children-match-ref`).
        // ALLOW(entity_uri_from_raw): set_field id &str from CrudOperations API surface
        let uri = EntityUri::from_raw(id);
        let routed = self
            .cell_registry
            .write_field(&uri, field, value.clone())
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("BlockCellRegistry::write_field({field}): {e:#}").into()
            })?;
        if routed {
            // The Loro outbound projector emits the SQL UPDATE and the resulting
            // CDC event produces the FieldDelta; there is no synchronous change
            // to surface here. This is the org-reingest / structural seam — user
            // CRUD `set_field` (which needs an undo inverse) is served by the
            // Loro CRUD authority (`LoroBlockOperations`) under Loro authority,
            // and by `SqlOperationProvider` in SqlOnly mode.
            return Ok(OperationResult::irreversible(Vec::new()));
        }

        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("field".into(), Value::String(field.to_string()));
        params.insert("value".into(), value);
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation(&entity, "set_field", params)
            .await
    }

    async fn create(&self, fields: holon_api::StorageEntity) -> Result<(String, OperationResult)> {
        let entity = EntityName::new(Block::entity_name());
        let id = fields
            .get("id")
            .and_then(|v| v.as_string())
            .map(String::from)
            .ok_or_else(|| "SqlBlockOperations::create: missing 'id'".to_string())?;

        // This path writes `block_raw` directly — it never goes through
        // `BlockOrdering::place`/`OrderKeyMinting`, so a caller that omits
        // `sort_key` (creation-slot commit, a bare `block.create` Rhai/MCP
        // action, page create) would otherwise fall through to the SQL
        // column's literal default `"A0"` for EVERY such create. Two
        // consecutive id-less creates under the same parent then collide on
        // the identical key, leaving sibling order ambiguous until some
        // later op (e.g. `split_block`'s tie-detected rebalance) re-mints
        // distinct keys. Mint a real key here — strictly after the current
        // last sibling — using the same `gen_key_between` fractional-index
        // generator `new_child_anchor` uses, so a caller-supplied `sort_key`
        // still wins.
        //
        // Gated on consolidator exactly like `new_child_anchor`: only the
        // SqlOnly order owner mints. In Upstream (Loro) mode the tree is
        // authoritative and its outbound projector is the sole `sort_key`
        // writer (see the UPSERT comment in `prepare_create`), so minting
        // here too would mix `gen_key_between` values with Loro-fi values in
        // the same column — the exact keyspace-mixing bug class invariant 10
        // warns about.
        let mut fields = fields;
        if !fields.contains_key("sort_key")
            && matches!(self.consolidator(), Consolidator::Store)
            && let Some(parent_id) = fields.get("parent_id").and_then(|v| v.as_string())
        {
            let siblings = self.sibling_keys(parent_id).await?;
            let last_key = siblings.last().map(|(_, sk)| sk.clone());
            // ALLOW(order_minting): sanctioned SqlOnly order-owner mint site
            // (Replication.md §5), same file/gate as `new_child_anchor`.
            let minted = gen_key_between(last_key.as_deref(), None).map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("SqlBlockOperations::create: mint sort_key: {e:#}").into()
                },
            )?;
            fields.insert("sort_key".into(), Value::String(minted));
        }

        let result = self
            .sql_ops
            .execute_operation(&entity, "create", fields)
            .await?;
        Ok((id, result))
    }

    async fn delete(&self, id: &str) -> Result<OperationResult> {
        // Fail-closed on NON-LEAF (destructive-delete ruling 2026-07-21): a bare
        // `delete` NEVER cascades a subtree. The caller must opt in explicitly
        // via `delete_subtree` or `delete_keep_children`. Mirrors the Loro
        // authority's guard so both providers refuse identically.
        // ALLOW(entity_uri_from_raw): op-dispatch id string → EntityUri at the edge
        let uri = EntityUri::from_raw(id);
        let children = self.children_ordered(&uri).await?;
        if !children.is_empty() {
            return Err(format!(
                "delete: block {id} has {} child(ren); refusing to cascade. Use \
                 `delete_subtree` to delete the whole subtree, or \
                 `delete_keep_children` to reparent the children first.",
                children.len()
            )
            .into());
        }

        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation(&entity, "delete", params)
            .await
    }
}

#[async_trait]
impl OperationProvider for SqlBlockOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        use holon_core::__operations_block_operations;
        let entity_name = Block::entity_name();
        let short_name = Block::short_name().expect("Block must have short_name");
        let id_column = "id";
        __operations_block_operations::block_operations(
            entity_name,
            short_name,
            entity_name,
            id_column,
        )
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        use holon_core::__operations_block_operations;

        if entity_name.as_str() != Block::entity_name() {
            return Err(format!(
                "SqlBlockOperations: expected entity '{}', got '{}'",
                Block::entity_name(),
                entity_name
            )
            .into());
        }

        match __operations_block_operations::dispatch_operation::<_, Block>(self, op_name, &params)
            .await
        {
            Ok(op) => Ok(op),
            Err(err) => {
                if UnknownOperationError::is_unknown(err.as_ref()) {
                    Err(format!("SqlBlockOperations: unknown block operation '{}'", op_name).into())
                } else {
                    Err(err)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use holon_api::block::Block;
    use holon_api::entity_uri::EntityUri;
    use holon_core::__operations_block_operations;
    use holon_core::OperationRegistry;
    use holon_core::block_ordering::BlockOrdering;
    use holon_core::storage::types::StorageEntity;
    use holon_turso::schema_modules::BlockMatviewSchemaModule;
    use holon_turso::schema_modules::BlockSchemaModule;

    use super::SqlBlockOperations;
    use crate::core::queryable_cache::QueryableCache;
    use crate::core::sql_operation_provider::SqlOperationProvider;
    use crate::storage::BLOCK_WRITE_TABLE;
    use crate::storage::schema_module::SchemaModule;
    use crate::storage::turso::TursoBackend;

    /// Sanity check: the macro-generated `block_operations()` descriptor
    /// list — what `SqlBlockOperations::operations` returns — advertises
    /// indent / outdent / move_block / move_up / move_down. On `main`
    /// (before this provider was registered), the dispatcher had no entry
    /// for `("block", "indent")`. See the diagnostic comment on
    /// `BLOCK_TREE_KEYCHORD_OPS_ENABLED` in
    /// crates/holon-integration-tests/src/pbt/state_machine.rs.
    #[test]
    fn block_operations_advertise_indent_and_outdent() {
        let entity_name = Block::entity_name();
        let short_name = Block::short_name().expect("Block must have short_name");
        let names: Vec<String> = __operations_block_operations::block_operations(
            entity_name,
            short_name,
            entity_name,
            "id",
        )
        .into_iter()
        .map(|d| d.name)
        .collect();
        for op in ["indent", "outdent", "move_block", "move_up", "move_down"] {
            assert!(names.iter().any(|n| n == op), "{op} missing: {names:?}");
        }
    }

    async fn setup_sql_block_ops() -> (
        TursoBackend,
        Arc<SqlBlockOperations>,
        crate::storage::turso::DbHandle,
    ) {
        let (backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory turso");
        handle
            .execute_ddl("PRAGMA foreign_keys = ON")
            .await
            .expect("FK pragma");
        // Minimal schema: block_raw + junction tables + block matview.
        holon_turso::schema_modules::CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("CoreSchemaModule");
        BlockSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("BlockSchemaModule");
        BlockMatviewSchemaModule
            .ensure_schema(&handle)
            .await
            .expect("BlockMatviewSchemaModule");

        let descriptors = BlockSchemaModule.edge_fields();
        let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
            handle.clone(),
            BLOCK_WRITE_TABLE.to_string(),
            "block".to_string(),
            "block".to_string(),
            descriptors,
        ));
        // Point the cache at `block_raw` (not the `block` matview) so that
        // test INSERTs into block_raw are immediately visible via get_by_id.
        let mut block_raw_type_def = Block::type_definition();
        block_raw_type_def.name = "block_raw".to_string();
        let cache = Arc::new(
            QueryableCache::<Block>::new(handle.clone(), block_raw_type_def)
                .await
                .expect("cache"),
        );
        let ops = Arc::new(SqlBlockOperations::new(sql_ops, cache));
        (backend, ops, handle)
    }

    async fn read_sort_key(handle: &crate::storage::turso::DbHandle, bare_id: &str) -> String {
        let rows = handle
            .query(
                &format!(
                    "SELECT sort_key FROM block_raw WHERE id = '{}'",
                    bare_id.replace('\'', "''")
                ),
                std::collections::HashMap::new(),
            )
            .await
            .expect("read sort_key");
        rows.into_iter()
            .next()
            .and_then(|r| {
                r.get("sort_key")
                    .and_then(|v| v.as_string().map(str::to_string))
            })
            .unwrap_or_default()
    }

    /// Test 6: `SqlBlockOperations::place` is idempotent in SqlOnly mode.
    ///
    /// Insert two siblings A, B (B sort_key > A). Call `place(B, parent,
    /// Some(A.id))` twice. Assert the sort_key of B does NOT change after the
    /// second call — the idempotency guard short-circuits when the current
    /// predecessor already matches the requested placement.
    #[tokio::test]
    async fn place_is_idempotent_in_sql_only_mode() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;

        // Use a real block as parent so the is_block() guard in prev_sibling
        // doesn't short-circuit — the idempotency check relies on prev_sibling.
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let parent = EntityUri::from_raw("block:test-parent");
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let a_id = EntityUri::from_raw("block:test-a");
        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let b_id = EntityUri::from_raw("block:test-b");

        // IDs stored in block_raw use full URI form (e.g. "block:test-a") so
        // EntityUri deserialization round-trips via QueryableCache.
        // Sort keys use valid fractional-index format (hex strings); "a0" < "a1".
        for (id, sort_key, content) in [
            (parent.as_str(), "V", "parent"),
            (a_id.as_str(), "a0", "A"),
            (b_id.as_str(), "a1", "B"),
        ] {
            let parent_val = if id == parent.as_str() {
                "sentinel:no_parent"
            } else {
                parent.as_str()
            };
            handle
                .execute(
                    &format!(
                        "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                         created_at, updated_at) VALUES ('{}', '{}', '{}', '{}', 'text', 0, 0)",
                        id, parent_val, sort_key, content
                    ),
                    vec![],
                )
                .await
                .unwrap_or_else(|e| panic!("insert {id}: {e}"));
        }

        // First placement: B is already after A (sort_key "a1" > "a0").
        // The idempotency guard detects this via prev_sibling and skips the write.
        // Pass full URI strings — place() compares against EntityUri::as_str().
        ops.place(&b_id, &parent, Some(&a_id))
            .await
            .expect("place B after A (first call)");

        let sort_key_after_first = read_sort_key(&handle, b_id.as_str()).await;

        // Second call — must be a no-op (same guard fires again).
        ops.place(&b_id, &parent, Some(&a_id))
            .await
            .expect("place B after A (second call)");

        let sort_key_after_second = read_sort_key(&handle, b_id.as_str()).await;

        assert_eq!(
            sort_key_after_first, sort_key_after_second,
            "place() called twice with identical args must not change sort_key (idempotency guard \
             regression)"
        );
    }

    /// Bug (dogfood 2026-07-10): `block.create` without an explicit `sort_key`
    /// always minted the SQL column default `"A0"`, so two consecutive
    /// id-less creates under the same parent collided on the identical key —
    /// sibling order stayed ambiguous until some later op (e.g.
    /// `split_block`'s tie-detected rebalance) re-minted distinct keys.
    /// `SqlBlockOperations::create` must instead mint a real, strictly
    /// increasing key for each create when the caller omits `sort_key`.
    #[tokio::test]
    async fn create_without_sort_key_mints_strictly_increasing_keys() {
        use std::collections::HashMap;

        use holon_core::CrudOperations;
        use holon_core::storage::types::StorageEntity;

        let (_backend, ops, handle) = setup_sql_block_ops().await;

        // ALLOW(entity_uri_from_raw): test-fixture literal (#[cfg(test)])
        let parent = EntityUri::from_raw("block:test-parent");
        handle
            .execute(
                &format!(
                    "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                     created_at, updated_at) VALUES ('{}', 'sentinel:no_parent', 'V', 'parent', \
                     'text', 0, 0)",
                    parent.as_str()
                ),
                vec![],
            )
            .await
            .expect("insert parent");

        let mut fields1: StorageEntity = HashMap::new();
        fields1.insert(
            "id".into(),
            holon_api::Value::String("block:child-1".to_string()),
        );
        fields1.insert(
            "parent_id".into(),
            holon_api::Value::String(parent.as_str().to_string()),
        );
        fields1.insert(
            "content".into(),
            holon_api::Value::String("first".to_string()),
        );
        fields1.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );
        ops.create(fields1).await.expect("create child 1");

        let mut fields2: StorageEntity = HashMap::new();
        fields2.insert(
            "id".into(),
            holon_api::Value::String("block:child-2".to_string()),
        );
        fields2.insert(
            "parent_id".into(),
            holon_api::Value::String(parent.as_str().to_string()),
        );
        fields2.insert(
            "content".into(),
            holon_api::Value::String("second".to_string()),
        );
        fields2.insert(
            "content_type".into(),
            holon_api::Value::String("text".to_string()),
        );
        ops.create(fields2).await.expect("create child 2");

        let key1 = read_sort_key(&handle, "block:child-1").await;
        let key2 = read_sort_key(&handle, "block:child-2").await;

        assert_ne!(
            key1, "A0",
            "id-less create must not fall back to the literal SQL default"
        );
        assert_ne!(
            key2, "A0",
            "id-less create must not fall back to the literal SQL default"
        );
        assert_ne!(
            key1, key2,
            "two consecutive id-less creates must not collide"
        );
        assert!(
            key1 < key2,
            "second create must sort strictly after the first: {key1:?} vs {key2:?}"
        );
    }

    /// Insert a parent row directly and return its full URI.
    async fn seed_parent(handle: &crate::storage::turso::DbHandle, bare: &str) -> String {
        let id = format!("block:{bare}");
        handle
            .execute(
                &format!(
                    "INSERT INTO block_raw (id, parent_id, sort_key, content, content_type, \
                     created_at, updated_at) VALUES ('{id}', 'sentinel:no_parent', 'V', 'parent', \
                     'text', 0, 0)"
                ),
                vec![],
            )
            .await
            .expect("insert parent");
        id
    }

    fn child_create_op(parent: &str, i: usize, after: Option<&str>) -> (String, StorageEntity) {
        use holon_api::Value;
        let mut p: StorageEntity = std::collections::HashMap::new();
        p.insert("id".into(), Value::String(format!("block:c{i}")));
        p.insert("parent_id".into(), Value::String(parent.to_string()));
        p.insert("content".into(), Value::String(format!("child {i}")));
        p.insert("content_type".into(), Value::String("text".to_string()));
        if let Some(a) = after {
            p.insert(
                crate::sync::event_bus::POSITION_AFTER_BLOCK_ID_PARAM.into(),
                Value::String(a.to_string()),
            );
        }
        ("create".to_string(), p)
    }

    /// Count matview-maintenance passes = broadcast batches on the `block`
    /// matview CDC channel. Turso emits CDC per matview per transaction commit
    /// (base tables emit none — see `cdc_base_vs_matview_repro`), so one batch
    /// == one IVM maintenance pass. Waits up to `first` for the first pass,
    /// then drains until the channel stays quiet for `quiet`.
    async fn count_matview_passes(
        cdc: &mut tokio::sync::broadcast::Receiver<
            holon_api::streaming::WithMetadata<
                holon_api::streaming::Batch<crate::storage::turso::RowChange>,
                holon_api::streaming::BatchMetadata,
            >,
        >,
        first: std::time::Duration,
        quiet: std::time::Duration,
    ) -> usize {
        let mut n = 0usize;
        if tokio::time::timeout(first, cdc.recv()).await.is_ok() {
            n += 1;
        } else {
            return n;
        }
        while tokio::time::timeout(quiet, cdc.recv()).await.is_ok() {
            n += 1;
        }
        n
    }

    const FIRST_WAIT: std::time::Duration = std::time::Duration::from_secs(3);
    const QUIET: std::time::Duration = std::time::Duration::from_millis(300);

    /// BugFunnel row 32 fix. Applying a whole file's ingest through
    /// `apply_ingest_batch` drives the live watch-view matview IVM maintenance
    /// exactly ONCE — one transaction commit per file — regardless of the block
    /// count, instead of once per block (the O(N²) cold-boot cost whose
    /// per-block price scaled with the accumulated table). The born-correct
    /// per-parent sort_keys are strictly increasing, so the downstream
    /// `place_all` totalizer would rewrite nothing.
    #[tokio::test(flavor = "multi_thread")]
    async fn batched_ingest_runs_matview_maintenance_once_per_file() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;
        let parent = seed_parent(&handle, "p").await;

        let mut cdc = handle
            .subscribe_cdc("block")
            .await
            .expect("subscribe block matview cdc");
        // Clear the parent-insert pass so the count reflects only the ingest.
        let _ = count_matview_passes(&mut cdc, FIRST_WAIT, QUIET).await;

        let n = 16usize;
        let mut prev: Option<String> = None;
        let ops_vec: Vec<(String, StorageEntity)> = (0..n)
            .map(|i| {
                let op = child_create_op(&parent, i, prev.as_deref());
                prev = Some(format!("block:c{i}"));
                op
            })
            .collect();

        ops.apply_ingest_batch(ops_vec)
            .await
            .expect("batched ingest");

        let passes = count_matview_passes(&mut cdc, FIRST_WAIT, QUIET).await;
        assert_eq!(
            passes, 1,
            "one matview IVM maintenance pass per FILE (row 32 O(N²) fix); got {passes} passes for \
             {n} blocks in one batch"
        );

        // Born-correct order: strictly increasing per-parent keys ⇒ place_all
        // no-op (never re-mints a single row).
        let mut keys = Vec::new();
        for i in 0..n {
            keys.push(read_sort_key(&handle, &format!("block:c{i}")).await);
        }
        for w in keys.windows(2) {
            assert!(
                w[0] < w[1],
                "batched creates must be born strictly increasing so place_all rewrites nothing: \
                 {keys:?}"
            );
        }
    }

    /// Baseline that the fix removes: the historic per-op apply drove ONE
    /// matview maintenance pass per block (N passes for N blocks).
    /// Documents that the batch count above is a real collapse, not an
    /// artifact of the metric.
    #[tokio::test(flavor = "multi_thread")]
    async fn per_op_ingest_runs_one_matview_pass_per_block() {
        let (_backend, ops, handle) = setup_sql_block_ops().await;
        let parent = seed_parent(&handle, "p").await;

        let mut cdc = handle
            .subscribe_cdc("block")
            .await
            .expect("subscribe block matview cdc");
        let _ = count_matview_passes(&mut cdc, FIRST_WAIT, QUIET).await;

        let n = 8usize;
        let mut prev: Option<String> = None;
        for i in 0..n {
            let (_op, params) = child_create_op(&parent, i, prev.as_deref());
            ops.update_in_tree(params).await.expect("per-op create");
            prev = Some(format!("block:c{i}"));
        }

        let passes = count_matview_passes(&mut cdc, FIRST_WAIT, QUIET).await;
        assert_eq!(
            passes, n,
            "the per-op path runs one matview maintenance pass per block ({n} expected); this is \
             the O(N²) cost the batch path collapses to 1"
        );
    }
}

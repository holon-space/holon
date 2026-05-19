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
//! authority) and the outbound projector emits the SQL row; in SqlOnly mode they
//! land in SQL directly via `SqlOperationProvider::execute_operation`. There is
//! no SQL→Loro reflection. Reads come from `QueryableCache<Block>` — same
//! backing store as the rest of the system.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::{EntityName, Tags, Value};

use crate::core::datasource::{
    BlockDataSourceHelpers, BlockMaintenanceHelpers, BlockOperations, BlockQueryHelpers,
    CrudOperations, DataSource, HasCache, OperationDescriptor, OperationProvider,
    OperationRegistry, OperationResult, Result, UnknownOperationError,
};
use crate::core::queryable_cache::QueryableCache;
use crate::core::sql_operation_provider::SqlOperationProvider;
use crate::storage::types::StorageEntity;
use crate::sync::block_cell_registry::BlockCellRegistry;
use crate::sync::event_bus::EventOrigin;
use holon_api::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::cell_registry::EntityCellRegistry;
use holon_core::fractional_index::{gen_key_between, gen_n_keys};

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
    async fn get_prev_sibling(&self, block_id: &str) -> Result<Option<Block>> {
        match <Self as BlockOrdering>::prev_sibling(self, block_id).await? {
            Some(id) => self.get_by_id(&id).await,
            None => Ok(None),
        }
    }

    async fn get_next_sibling(&self, block_id: &str) -> Result<Option<Block>> {
        match <Self as BlockOrdering>::next_sibling(self, block_id).await? {
            Some(id) => self.get_by_id(&id).await,
            None => Ok(None),
        }
    }

    async fn get_first_child(&self, parent_id: Option<&str>) -> Result<Option<Block>> {
        let Some(pid) = parent_id else {
            return Ok(None);
        };
        match <Self as BlockOrdering>::first_child(self, pid).await? {
            Some(id) => self.get_by_id(&id).await,
            None => Ok(None),
        }
    }

    async fn get_last_child(&self, parent_id: Option<&str>) -> Result<Option<Block>> {
        let Some(pid) = parent_id else {
            return Ok(None);
        };
        match <Self as BlockOrdering>::last_child(self, pid).await? {
            Some(id) => self.get_by_id(&id).await,
            None => Ok(None),
        }
    }
}
impl BlockMaintenanceHelpers<Block> for SqlBlockOperations {}
impl BlockDataSourceHelpers<Block> for SqlBlockOperations {}
impl BlockOperations<Block> for SqlBlockOperations {
    fn cells(&self) -> Option<&dyn EntityCellRegistry> {
        Some(&*self.cell_registry as &dyn EntityCellRegistry)
    }

    fn ordering(&self) -> Option<&dyn BlockOrdering> {
        Some(self as &dyn BlockOrdering)
    }
}

#[async_trait]
impl BlockOrdering for SqlBlockOperations {
    /// Loro mode → `write_position` (tree.mov_after). SqlOnly mode →
    /// `new_child_anchor` + paired `set_field("parent_id") +
    /// set_field("sort_key")` via the underlying `OperationProvider`.
    async fn place(&self, uri: &EntityUri, parent_id: &str, after_id: Option<&str>) -> Result<()> {
        if self
            .cell_registry
            .write_position(uri, parent_id, after_id)
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
                let current_prev = self.prev_sibling(uri.as_str()).await.map_err(
                    |e| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("place: prev_sibling {}: {e:#}", uri.as_str()).into()
                    },
                )?;
                if current_block.parent_id.as_str() == parent_id
                    && current_prev.as_deref() == after_id
                {
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
        parent_params.insert("id".to_string(), Value::String(id.to_string()));
        parent_params.insert("field".to_string(), Value::String("parent_id".to_string()));
        parent_params.insert("value".to_string(), Value::String(parent_id.to_string()));
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation(&entity, "set_field", parent_params)
            .await?;
        let mut sort_params: StorageEntity = HashMap::new();
        sort_params.insert("id".to_string(), Value::String(id.to_string()));
        sort_params.insert("field".to_string(), Value::String("sort_key".to_string()));
        sort_params.insert("value".to_string(), Value::String(new_sort_key));
        self.sql_ops
            .execute_operation(&entity, "set_field", sort_params)
            .await?;
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
        requires: &[String],
    ) -> Result<bool> {
        self.cell_registry
            .create_entity(
                parent_id, after_id, new_id, content, properties, tags, requires,
            )
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    /// Apply a block update intent — the single org→block mutation seam (no
    /// command bus behind it).
    ///
    /// Currently SQL-first (delegates to the SQL operation provider, which
    /// partitions edge fields and lifts `POSITION_AFTER_BLOCK_ID_PARAM`). Picks
    /// `create`/`update` by the row's prior presence so the emitted CDC kind
    /// matches.
    ///
    /// KNOWN GAP: now that the SQL→Loro mirror is gone, a SQL-first update never
    /// reaches Loro, so the Loro→SQL projection can revert an org content-edit to
    /// Loro's stale value on a later reconcile. The fix is to route this path
    /// Loro-first (like `create_in_tree`), but doing so deterministically
    /// regresses sibling ordering via the org place loop (the
    /// `inv-live-children` `ref-doc-3` divergence) — convergence and ordering are
    /// coupled through the place loop in a not-yet-understood way. Routing updates
    /// Loro-first belongs with the single-owner-order rewrite (tasks #10/#11).
    async fn update_in_tree(&self, params: HashMap<String, Value>) -> Result<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .map(str::to_string)
            .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                "update_in_tree: missing 'id' param".into()
            })?;
        let op = if self.cache.get_by_id(&id).await?.is_some() {
            "update"
        } else {
            "create"
        };
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation_with_origin(&entity, op, params, EventOrigin::Org)
            .await?;
        Ok(())
    }

    /// Apply a block delete intent.
    ///
    /// Loro mode: delete from the Loro tree (the authority) via the cell
    /// registry; the outbound projector emits the SQL DELETE. This mirrors
    /// `create_in_tree` (creates go to Loro, the projector writes SQL). Deleting
    /// only from SQL would race the armed projection, which re-creates the
    /// still-present Loro node back into SQL — the block resurrects (observed as
    /// `inv-backend-blocks-match-ref` spurious `bulk-*` rows).
    ///
    /// SqlOnly mode: the registry returns `false`; delete straight from SQL via
    /// the operation provider, preserving the `ROUTING_DOC_URI_KEY` hint so
    /// `prepare_delete` skips the recursive document walk.
    async fn delete_in_tree(&self, params: HashMap<String, Value>) -> Result<()> {
        let id = params.get("id").and_then(|v| v.as_string()).ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                "delete_in_tree: missing 'id' param".into()
            },
        )?;
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
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation_with_origin(&entity, "delete", params, EventOrigin::Org)
            .await?;
        Ok(())
    }

    fn is_loro_backed(&self) -> bool {
        self.cell_registry.is_loro_backed()
    }

    /// Loro mode → read each block's live fractional index from the Loro tree
    /// and write it to SQL `sort_key` via the standard `set_field` path,
    /// mirroring the boot-time seed writeback
    /// (`loro_module::seed_loro_from_persistent_store`). This closes the
    /// projection-totality gap: a block created but never repositioned emits no
    /// Loro mov delta, so the outbound projector never writes its fi and it
    /// keeps the default `"A0"`. The write goes through
    /// `SqlOperationProvider::set_field` (→ `prepare_update`) rather than a raw
    /// `UPDATE block_raw`, so the `properties` column is read-merged and
    /// re-canonicalised (`properties_to_canonical_json`) — a bare single-column
    /// raw update desyncs the matview's `properties` projection
    /// (the `props_check` invariant's "Value::Object serialization bug").
    /// SqlOnly mode → no-op (SQL owns `sort_key`; `live_sort_key` returns
    /// `None`).
    async fn project_sort_keys(&self, ids: &[&str]) -> Result<()> {
        let entity = EntityName::new(Block::entity_name());
        for id in ids {
            let fi = match self.cell_registry.live_sort_key(id).await.map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> {
                    format!("project_sort_keys: live_sort_key({id}): {e:#}").into()
                },
            )? {
                Some(fi) => fi,
                None => return Ok(()), // SqlOnly — SQL already owns sort_key
            };
            let mut params: StorageEntity = HashMap::new();
            params.insert("id".to_string(), Value::String(id.to_string()));
            params.insert("field".to_string(), Value::String("sort_key".to_string()));
            params.insert("value".to_string(), Value::String(fi));
            self.sql_ops
                .execute_operation(&entity, "set_field", params)
                .await?;
        }
        Ok(())
    }

    /// Returns the SQL `block.sort_key` value to persist for a new block
    /// placed under `parent_id` after `after_id`. Computed unconditionally
    /// via `gen_key_between` against the neighbor sort_keys in the cache —
    /// in Loro mode this value is silently overwritten by `apply_create`
    /// after `Event::position_after_block_id` drives `tree.mov_after`,
    /// so the unused compute is the price of avoiding a separate Loro
    /// vs SqlOnly conditional here.
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
    async fn new_child_anchor(&self, parent_id: &str, after_id: Option<&str>) -> Result<String> {
        let blocks = self.cache.get_all().await?;
        let mut siblings: Vec<&Block> = blocks
            .iter()
            .filter(|b| b.parent_id.as_str() == parent_id)
            .collect();
        // Match `OrgRenderer::render_entity_tree` ordering: `(sort_key, id)`
        // lexicographic. The new block's "after" slot is interpreted in
        // this same order so the on-disk render matches the intent.
        siblings.sort_by(|a, b| {
            a.sort_key
                .cmp(&b.sort_key)
                .then_with(|| a.id.as_str().cmp(b.id.as_str()))
        });

        let has_ties = siblings.windows(2).any(|w| w[0].sort_key == w[1].sort_key);

        if has_ties {
            // Insertion index: where the new block lands in the rebalanced
            // sequence. With no `after_id` it's slot 0 (first child).
            let insert_idx = match after_id {
                None => 0usize,
                Some(after) => {
                    siblings
                        .iter()
                        .position(|b| b.id.as_str() == after)
                        .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                            format!("new_child_anchor: after block {after} missing").into()
                        })?
                        + 1
                }
            };
            let new_keys = gen_n_keys(siblings.len() + 1).map_err(
                |e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() },
            )?;
            let new_block_key = new_keys[insert_idx].clone();
            let entity = EntityName::new(Block::entity_name());
            for (i, sibling) in siblings.iter().enumerate() {
                let target_key = if i < insert_idx {
                    &new_keys[i]
                } else {
                    &new_keys[i + 1]
                };
                if &sibling.sort_key == target_key {
                    continue;
                }
                let mut params: StorageEntity = HashMap::new();
                params.insert(
                    "id".to_string(),
                    Value::String(sibling.id.as_str().to_string()),
                );
                params.insert("field".to_string(), Value::String("sort_key".to_string()));
                params.insert("value".to_string(), Value::String(target_key.clone()));
                self.sql_ops
                    .execute_operation(&entity, "set_field", params)
                    .await?;
            }
            return Ok(new_block_key);
        }

        let (prev_key, next_key): (Option<String>, Option<String>) = match after_id {
            None => {
                let first = siblings.first().map(|b| b.sort_key.clone());
                (None, first)
            }
            Some(after) => {
                let after_block = siblings
                    .iter()
                    .find(|b| b.id.as_str() == after)
                    .ok_or_else(|| -> Box<dyn std::error::Error + Send + Sync> {
                        format!("new_child_anchor: after block {after} missing").into()
                    })?;
                let next = siblings
                    .iter()
                    .find(|b| b.sort_key > after_block.sort_key)
                    .map(|b| b.sort_key.clone());
                (Some(after_block.sort_key.clone()), next)
            }
        };
        gen_key_between(prev_key.as_deref(), next_key.as_deref())
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }

    async fn prev_sibling(&self, id: &str) -> Result<Option<String>> {
        let blocks = self.cache.get_all().await?;
        let block = blocks.iter().find(|b| b.id.as_str() == id).ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                format!("prev_sibling: block {id} missing").into()
            },
        )?;
        if !block.parent_id.is_block() {
            return Ok(None);
        }
        let parent_id = block.parent_id.as_str();
        Ok(blocks
            .iter()
            .filter(|b| b.parent_id.as_str() == parent_id)
            .filter(|b| b.sort_key < block.sort_key)
            .max_by(|a, b| a.sort_key.cmp(&b.sort_key))
            .map(|b| b.id.as_str().to_string()))
    }

    async fn next_sibling(&self, id: &str) -> Result<Option<String>> {
        let blocks = self.cache.get_all().await?;
        let block = blocks.iter().find(|b| b.id.as_str() == id).ok_or_else(
            || -> Box<dyn std::error::Error + Send + Sync> {
                format!("next_sibling: block {id} missing").into()
            },
        )?;
        if !block.parent_id.is_block() {
            return Ok(None);
        }
        let parent_id = block.parent_id.as_str();
        Ok(blocks
            .iter()
            .filter(|b| b.parent_id.as_str() == parent_id)
            .filter(|b| b.sort_key > block.sort_key)
            .min_by(|a, b| a.sort_key.cmp(&b.sort_key))
            .map(|b| b.id.as_str().to_string()))
    }

    async fn first_child(&self, parent_id: &str) -> Result<Option<String>> {
        let blocks = self.cache.get_all().await?;
        Ok(blocks
            .iter()
            .filter(|b| b.parent_id.as_str() == parent_id)
            .min_by(|a, b| a.sort_key.cmp(&b.sort_key))
            .map(|b| b.id.as_str().to_string()))
    }

    async fn last_child(&self, parent_id: &str) -> Result<Option<String>> {
        let blocks = self.cache.get_all().await?;
        Ok(blocks
            .iter()
            .filter(|b| b.parent_id.as_str() == parent_id)
            .max_by(|a, b| a.sort_key.cmp(&b.sort_key))
            .map(|b| b.id.as_str().to_string()))
    }

    async fn children(&self, parent_id: &str) -> Result<Vec<String>> {
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
            return Ok(kids);
        }
        let blocks = self.cache.get_all().await?;
        let mut kids: Vec<&Block> = blocks
            .iter()
            .filter(|b| b.parent_id.as_str() == parent_id)
            .collect();
        kids.sort_by(|a, b| a.sort_key.cmp(&b.sort_key));
        Ok(kids
            .into_iter()
            .map(|b| b.id.as_str().to_string())
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
        let uri = EntityUri::block(id);
        let routed = self
            .cell_registry
            .write_field(&uri, field, value.clone())
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("BlockCellRegistry::write_field({field}): {e:#}").into()
            })?;
        if routed {
            // The Loro outbound projector will emit the SQL UPDATE and the
            // resulting CDC event will produce the FieldDelta. From this
            // call site there are no SQL changes to surface synchronously.
            return Ok(OperationResult::irreversible(Vec::new()));
        }

        let mut params: StorageEntity = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        params.insert("field".to_string(), Value::String(field.to_string()));
        params.insert("value".to_string(), value);
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation(&entity, "set_field", params)
            .await
    }

    async fn create(&self, fields: HashMap<String, Value>) -> Result<(String, OperationResult)> {
        let entity = EntityName::new(Block::entity_name());
        let id = fields
            .get("id")
            .and_then(|v| v.as_string())
            .map(String::from)
            .ok_or_else(|| "SqlBlockOperations::create: missing 'id'".to_string())?;
        let result = self
            .sql_ops
            .execute_operation(&entity, "create", fields)
            .await?;
        Ok((id, result))
    }

    async fn delete(&self, id: &str) -> Result<OperationResult> {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".to_string(), Value::String(id.to_string()));
        let entity = EntityName::new(Block::entity_name());
        self.sql_ops
            .execute_operation(&entity, "delete", params)
            .await
    }
}

#[async_trait]
impl OperationProvider for SqlBlockOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        use crate::core::datasource::__operations_block_operations;
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
        use crate::core::datasource::__operations_block_operations;

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
    use super::SqlBlockOperations;
    use crate::core::datasource::{__operations_block_operations, OperationRegistry};
    use crate::core::queryable_cache::QueryableCache;
    use crate::core::sql_operation_provider::SqlOperationProvider;
    use crate::storage::BLOCK_WRITE_TABLE;
    use crate::storage::schema_module::SchemaModule;
    use crate::storage::schema_modules::{BlockMatviewSchemaModule, BlockSchemaModule};
    use crate::storage::turso::TursoBackend;
    use holon_api::block::Block;
    use holon_api::entity_uri::EntityUri;
    use holon_core::block_ordering::BlockOrdering;
    use std::sync::Arc;

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
        crate::storage::schema_modules::CoreSchemaModule
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
        let parent = EntityUri::from_raw("block:test-parent");
        let a_id = EntityUri::from_raw("block:test-a");
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
                        "INSERT INTO block_raw \
                         (id, parent_id, sort_key, content, content_type, created_at, updated_at) \
                         VALUES ('{}', '{}', '{}', '{}', 'text', 0, 0)",
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
        ops.place(&b_id, parent.as_str(), Some(a_id.as_str()))
            .await
            .expect("place B after A (first call)");

        let sort_key_after_first = read_sort_key(&handle, b_id.as_str()).await;

        // Second call — must be a no-op (same guard fires again).
        ops.place(&b_id, parent.as_str(), Some(a_id.as_str()))
            .await
            .expect("place B after A (second call)");

        let sort_key_after_second = read_sort_key(&handle, b_id.as_str()).await;

        assert_eq!(
            sort_key_after_first, sort_key_after_second,
            "place() called twice with identical args must not change sort_key \
             (idempotency guard regression)"
        );
    }
}

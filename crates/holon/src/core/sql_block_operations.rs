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
//! Each call is forwarded to `SqlOperationProvider::execute_operation`,
//! preserving the "SQL is source of truth" model: each `set_field` lands
//! in SQL, emits a CDC event, and reaches Loro through
//! `LoroSyncController::on_inbound_event`. Reads come from
//! `QueryableCache<Block>` — same backing store as the rest of the system.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::block::Block;
use holon_api::{EntityName, Value};

use crate::core::datasource::{
    BlockDataSourceHelpers, BlockMaintenanceHelpers, BlockOperations, BlockQueryHelpers,
    CrudOperations, DataSource, HasCache, OperationDescriptor, OperationProvider,
    OperationRegistry, OperationResult, Result, UnknownOperationError,
};
use crate::core::queryable_cache::QueryableCache;
use crate::core::sql_operation_provider::SqlOperationProvider;
use crate::storage::types::StorageEntity;
use crate::sync::block_cell_registry::BlockCellRegistry;
use holon_api::EntityUri;
use holon_core::block_ordering::BlockOrdering;
use holon_core::cell_registry::EntityCellRegistry;
use holon_core::fractional_index::gen_key_between;

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
        let new_sort_key = self.new_child_anchor(parent_id, after_id).await?;
        let id = uri.id();
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

    /// Returns the SQL `block.sort_key` value to persist for a new block
    /// placed under `parent_id` after `after_id`. Computed unconditionally
    /// via `gen_key_between` against the neighbor sort_keys in the cache —
    /// in Loro mode this value is silently overwritten by `apply_create`
    /// after `Event::position_after_block_id` drives `tree.mov_after`,
    /// so the unused compute is the price of avoiding a separate Loro
    /// vs SqlOnly conditional here.
    async fn new_child_anchor(&self, parent_id: &str, after_id: Option<&str>) -> Result<String> {
        let blocks = self.cache.get_all().await?;
        let (prev_key, next_key): (Option<String>, Option<String>) = match after_id {
            None => {
                let first = blocks
                    .iter()
                    .filter(|b| b.parent_id.as_str() == parent_id)
                    .min_by(|a, b| a.sort_key.cmp(&b.sort_key))
                    .map(|b| b.sort_key.clone());
                (None, first)
            }
            Some(after) => {
                let after_block = blocks.iter().find(|b| b.id.as_str() == after).ok_or_else(
                    || -> Box<dyn std::error::Error + Send + Sync> {
                        format!("new_child_anchor: after block {after} missing").into()
                    },
                )?;
                let next = blocks
                    .iter()
                    .filter(|b| b.parent_id == after_block.parent_id)
                    .filter(|b| b.sort_key > after_block.sort_key)
                    .min_by(|a, b| a.sort_key.cmp(&b.sort_key))
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
    use crate::core::datasource::{__operations_block_operations, OperationRegistry};
    use holon_api::block::Block;

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
}

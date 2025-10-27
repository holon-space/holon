//! Generic operations on Loro blocks.
//!
//! This is the primary mutation API for Loro. It's independent of any
//! specific persistence format (org-mode, JSON, etc.) and delegates to
//! `LoroBackend` for the actual tree operations.
//!
//! ## Change propagation
//!
//! Change propagation to the rest of the system is handled by
//! `LoroSyncController`, which subscribes to `doc.subscribe_root` on the
//! underlying `LoroDoc`. That subscription fires for **every** mutation —
//! whether it came through `LoroBlockOperations::{create,update,delete}`,
//! a raw `doc.import(&delta)`, a startup `.loro` load, or an offline
//! background-service merge. `LoroBlockOperations` itself does not emit
//! CDC events; the watermark on `LoroSyncController` is the single source
//! of truth for "what has been propagated."

use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

use holon_api::block::{Block, BlockContent};
use holon_api::{ContentType, EntityName, Value};

use crate::api::{CoreOperations, LoroBackend};
use crate::core::datasource::{
    BlockDataSourceHelpers, BlockMaintenanceHelpers, BlockOperations, BlockQueryHelpers,
    CompletionStateInfo, CrudOperations, DataSource, HasCache, MarkOperations, OperationDescriptor,
    OperationProvider, OperationRegistry, OperationResult, Result, TaskOperations, TextOperations,
    UnknownOperationError,
};
use crate::core::queryable_cache::QueryableCache;
use crate::storage::types::StorageEntity;
use crate::sync::LoroDocumentStore;

/// Generic operations on Loro blocks.
///
/// Implements standard operation traits, delegating to `LoroBackend`.
pub struct LoroBlockOperations {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    cache: Arc<QueryableCache<Block>>,
}

impl LoroBlockOperations {
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        cache: Arc<QueryableCache<Block>>,
    ) -> Self {
        Self { doc_store, cache }
    }

    /// Get the shared doc store (same instance used for writes).
    pub fn shared_doc_store(&self) -> Arc<RwLock<LoroDocumentStore>> {
        self.doc_store.clone()
    }

    /// Get the global backend (single LoroDoc for all blocks).
    async fn get_backend(&self, _doc_id: &str) -> Result<LoroBackend> {
        let store = self.doc_store.read().await;
        let collab_doc = store
            .get_global_doc()
            .await
            .map_err(|e| format!("Failed to get global doc: {}", e))?;
        Ok(LoroBackend::from_document(collab_doc))
    }

    /// Find the backend containing a block (always the global backend).
    async fn find_doc_for_block(&self, _block_id: &str) -> Result<(String, LoroBackend)> {
        let backend = self.get_backend("").await?;
        Ok((backend.doc_id().to_string(), backend))
    }

    /// Save the global document after modification.
    async fn save_doc(&self, _doc_path: &str) -> Result<()> {
        let store = self.doc_store.read().await;
        store.save_all().await?;
        Ok(())
    }
}

#[async_trait]
impl DataSource<Block> for LoroBlockOperations {
    async fn get_all(&self) -> Result<Vec<Block>> {
        self.cache.get_all().await
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Block>> {
        self.cache.get_by_id(id).await
    }
}

#[async_trait]
impl HasCache<Block> for LoroBlockOperations {
    fn get_cache(&self) -> &QueryableCache<Block> {
        &self.cache
    }
}

impl BlockQueryHelpers<Block> for LoroBlockOperations {}
impl BlockMaintenanceHelpers<Block> for LoroBlockOperations {}
impl BlockDataSourceHelpers<Block> for LoroBlockOperations {}
impl BlockOperations<Block> for LoroBlockOperations {}

#[async_trait]
impl CrudOperations<Block> for LoroBlockOperations {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
        let (doc_path, backend) = self.find_doc_for_block(id).await?;

        match field {
            "content" => {
                match &value {
                    // Plain string ⇒ text update; clears any existing marks.
                    Value::String(s) => {
                        backend
                            .update_block_text(id, s)
                            .await
                            .map_err(|e| format!("Failed to update content: {}", e))?;
                    }
                    // Object { text, marks } ⇒ rich update; applies marks via Peritext.
                    Value::Object(obj) => {
                        let text = obj
                            .get("text")
                            .and_then(|v| v.as_string())
                            .ok_or_else(|| {
                                "set_field('content', Object): missing 'text' string field"
                                    .to_string()
                            })?
                            .to_string();
                        let marks_json = obj.get("marks").and_then(|v| v.as_string()).ok_or_else(
                            || {
                                "set_field('content', Object): missing 'marks' JSON string field"
                                    .to_string()
                            },
                        )?;
                        let marks: Vec<holon_api::MarkSpan> =
                            holon_api::marks_from_json(marks_json).map_err(|e| {
                                format!("set_field('content'): marks JSON parse error: {e}")
                            })?;
                        backend
                            .update_block_marked(id, &text, &marks)
                            .await
                            .map_err(|e| format!("Failed to update marked content: {}", e))?;
                    }
                    other => {
                        return Err(format!(
                            "set_field('content'): unsupported value shape {other:?}"
                        )
                        .into());
                    }
                }
            }
            "marks" => {
                // Mark-only update: keep existing text, replace mark set.
                let marks_json = value
                    .as_string()
                    .ok_or_else(|| "set_field('marks'): expected JSON string Value".to_string())?;
                let marks: Vec<holon_api::MarkSpan> = holon_api::marks_from_json(marks_json)
                    .map_err(|e| format!("set_field('marks'): JSON parse error: {e}"))?;
                // Read current text from the backend; update_block_marked rewrites both.
                let current = backend
                    .get_block(id)
                    .await
                    .map_err(|e| format!("set_field('marks'): get_block: {e}"))?;
                backend
                    .update_block_marked(id, &current.content, &marks)
                    .await
                    .map_err(|e| format!("Failed to update marks: {}", e))?;
            }
            _ => {
                // Store in properties
                let mut props = HashMap::new();
                props.insert(field.to_string(), value);
                backend
                    .update_block_properties(id, &props)
                    .await
                    .map_err(|e| format!("Failed to update property: {}", e))?;
            }
        }

        self.save_doc(&doc_path).await?;

        // Propagation to downstream consumers is handled by `LoroSyncController`
        // via `doc.subscribe_root`.
        Ok(OperationResult::irreversible(vec![]))
    }

    async fn create(&self, fields: HashMap<String, Value>) -> Result<(String, OperationResult)> {
        // parent_id is required - it's either a document URI or a block ID
        let parent_id = fields
            .get("parent_id")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .ok_or_else(|| "parent_id is required for block creation")?;

        // All blocks live in the single global tree
        let doc_id = String::new();

        let content = fields
            .get("content")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .unwrap_or_default();

        let content_type: ContentType = fields
            .get("content_type")
            .and_then(|v| v.as_string())
            .unwrap_or("text")
            .parse()
            .expect("Invalid content_type in fields");

        let source_language = fields
            .get("source_language")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string());

        let block_id = fields
            .get("id")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string());

        tracing::debug!(
            "[LoroBlockOperations::create] doc_id={:?}, block_id={:?}, parent_id={:?}, content_type={:?}, source_language={:?}",
            doc_id,
            block_id,
            parent_id,
            content_type,
            source_language
        );

        // Build the appropriate BlockContent based on content_type
        let block_content = if content_type == ContentType::Source {
            let lang = source_language.as_deref().unwrap_or("text");
            BlockContent::source(lang, content.clone())
        } else {
            BlockContent::text(content.clone())
        };

        let backend = self.get_backend(&doc_id).await?;

        // Check if block already exists (upsert behavior)
        let existing_block = if let Some(ref id) = block_id {
            backend.get_block(id).await.ok() // ALLOW(ok): block may not exist
        } else {
            None
        };

        let block = if let Some(existing) = existing_block {
            // Block exists - update it instead of creating
            tracing::debug!(
                "[LoroBlockOperations::create] Block {} exists, updating instead",
                existing.id
            );

            // If parent changed, move the block in the tree
            let new_parent_ref = holon_api::EntityUri::from_raw(&parent_id);
            if existing.parent_id != new_parent_ref {
                backend
                    .move_block(existing.id.as_str(), new_parent_ref.clone(), None)
                    .await
                    .map_err(|e| format!("Failed to move block to new parent: {}", e))?;
            }

            backend
                .update_block(existing.id.as_str(), block_content.clone())
                .await
                .map_err(|e| format!("Failed to update existing block: {}", e))?;
            backend
                .get_block(existing.id.as_str())
                .await
                .map_err(|e| format!("Failed to get updated block: {}", e))?
        } else {
            // Block doesn't exist - create it
            let parent_uri = holon_api::EntityUri::from_raw(&parent_id);
            let block_uri = block_id.map(|id| holon_api::EntityUri::from_raw(&id));
            backend
                .create_block(parent_uri, block_content, block_uri)
                .await
                .map_err(|e| format!("Failed to create block: {}", e))?
        };

        // Set additional properties (excluding fields handled above and source block fields)
        let mut props = HashMap::new();
        let handled_fields = [
            "parent_id",
            "content",
            "id",
            "content_type",
            "source_language",
            "source_name",
            "source_header_args",
            "source_results",
        ];
        for (key, value) in &fields {
            if !handled_fields.contains(&key.as_str()) {
                props.insert(key.clone(), value.clone());
            }
        }
        if !props.is_empty() {
            backend
                .update_block_properties(block.id.as_str(), &props)
                .await
                .map_err(|e| format!("Failed to set properties: {}", e))?;
        }

        // Save
        self.save_doc(&doc_id).await?;

        // Re-fetch the block to get updated properties
        let block_with_props = backend
            .get_block(block.id.as_str())
            .await
            .map_err(|e| format!("Failed to get block after property update: {}", e))?;

        Ok((
            block_with_props.id.to_string(),
            OperationResult::irreversible(vec![]),
        ))
    }

    async fn delete(&self, id: &str) -> Result<OperationResult> {
        let (doc_path, backend) = self.find_doc_for_block(id).await?;

        backend
            .delete_block(id)
            .await
            .map_err(|e| format!("Failed to delete block: {}", e))?;

        self.save_doc(&doc_path).await?;

        Ok(OperationResult::irreversible(vec![]))
    }
}

impl LoroBlockOperations {
    /// Update a block with the given fields.
    ///
    /// Forwards to `create` which does upsert (create if not exists, update if exists).
    async fn update_block(&self, fields: HashMap<String, Value>) -> Result<OperationResult> {
        let (_block_id, result) = self.create(fields).await?;
        Ok(result)
    }
}

#[async_trait]
impl TaskOperations<Block> for LoroBlockOperations {
    async fn set_title(&self, id: &str, title: &str) -> Result<OperationResult> {
        // Get current content, replace first line
        if let Some(block) = self.cache.get_by_id(id).await? {
            let body: String = block.content.lines().skip(1).collect::<Vec<_>>().join("\n");
            let new_content = if body.is_empty() {
                title.to_string()
            } else {
                format!("{}\n{}", title, body)
            };
            self.set_field(id, "content", Value::String(new_content))
                .await
        } else {
            Err(format!("Block not found: {}", id).into())
        }
    }

    fn completion_states_with_progress(&self) -> Vec<CompletionStateInfo> {
        vec![
            CompletionStateInfo {
                state: "TODO".into(),
                progress: 0.0,
                is_done: false,
                is_active: true,
            },
            CompletionStateInfo {
                state: "DOING".into(),
                progress: 0.5,
                is_done: false,
                is_active: true,
            },
            CompletionStateInfo {
                state: "DONE".into(),
                progress: 1.0,
                is_done: true,
                is_active: false,
            },
        ]
    }

    async fn set_state(&self, id: &str, state: String) -> Result<OperationResult> {
        self.set_field(id, "TODO", Value::String(state)).await
    }

    async fn cycle_task_state(&self, id: &str) -> Result<OperationResult> {
        let block = self
            .cache
            .get_by_id(id)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Block not found: {id}"))?;
        let current = block.get_property_str("task_state").unwrap_or_default();
        let states: Vec<String> = std::iter::once(String::new())
            .chain(
                self.completion_states_with_progress()
                    .into_iter()
                    .map(|s| s.state),
            )
            .collect();
        let next = holon_api::render_eval::cycle_state(&current, &states);
        self.set_state(id, next).await
    }

    async fn set_due_date(
        &self,
        id: &str,
        date: Option<chrono::DateTime<chrono::Utc>>,
    ) -> Result<OperationResult> {
        match date {
            Some(dt) => {
                self.set_field(id, "DEADLINE", Value::String(dt.to_rfc3339()))
                    .await
            }
            None => self.set_field(id, "DEADLINE", Value::Null).await,
        }
    }

    async fn set_priority(&self, id: &str, priority: i64) -> Result<OperationResult> {
        self.set_field(id, "PRIORITY", Value::Integer(priority))
            .await
    }
}

#[async_trait]
impl MarkOperations<Block> for LoroBlockOperations {
    async fn apply_mark(
        &self,
        id: &str,
        range_start: i64,
        range_end: i64,
        mark_json: String,
    ) -> Result<OperationResult> {
        let start = usize::try_from(range_start).map_err(|_| {
            format!("apply_mark: range_start must be non-negative, got {range_start}")
        })?;
        let end = usize::try_from(range_end)
            .map_err(|_| format!("apply_mark: range_end must be non-negative, got {range_end}"))?;
        if start > end {
            return Err(
                format!("apply_mark: range_start ({start}) must be <= range_end ({end})").into(),
            );
        }
        let mark: holon_api::InlineMark = serde_json::from_str(&mark_json).map_err(|e| {
            format!("apply_mark: mark_json parse error: {e}; payload was: {mark_json}")
        })?;

        let (doc_path, backend) = self.find_doc_for_block(id).await?;
        backend
            .apply_inline_mark(id, start..end, &mark)
            .await
            .map_err(|e| format!("apply_inline_mark: {e}"))?;
        self.save_doc(&doc_path).await?;
        Ok(OperationResult::irreversible(vec![]))
    }

    async fn remove_mark(
        &self,
        id: &str,
        range_start: i64,
        range_end: i64,
        key: String,
    ) -> Result<OperationResult> {
        let start = usize::try_from(range_start).map_err(|_| {
            format!("remove_mark: range_start must be non-negative, got {range_start}")
        })?;
        let end = usize::try_from(range_end)
            .map_err(|_| format!("remove_mark: range_end must be non-negative, got {range_end}"))?;
        if start > end {
            return Err(
                format!("remove_mark: range_start ({start}) must be <= range_end ({end})").into(),
            );
        }
        if !holon_api::InlineMark::all_loro_keys().contains(&key.as_str()) {
            return Err(format!(
                "remove_mark: unknown mark key '{key}'; expected one of {:?}",
                holon_api::InlineMark::all_loro_keys()
            )
            .into());
        }

        let (doc_path, backend) = self.find_doc_for_block(id).await?;
        backend
            .remove_inline_mark(id, start..end, &key)
            .await
            .map_err(|e| format!("remove_inline_mark: {e}"))?;
        self.save_doc(&doc_path).await?;
        Ok(OperationResult::irreversible(vec![]))
    }
}

#[async_trait]
impl TextOperations<Block> for LoroBlockOperations {
    async fn insert_text(&self, id: &str, pos: i64, text: String) -> Result<OperationResult> {
        let pos = usize::try_from(pos)
            .map_err(|_| format!("insert_text: pos must be non-negative, got {pos}"))?;
        let (doc_path, backend) = self.find_doc_for_block(id).await?;
        backend
            .insert_text(id, pos, &text)
            .await
            .map_err(|e| format!("insert_text: {e}"))?;
        self.save_doc(&doc_path).await?;
        Ok(OperationResult::irreversible(vec![]))
    }

    async fn delete_text(&self, id: &str, pos: i64, len: i64) -> Result<OperationResult> {
        let pos = usize::try_from(pos)
            .map_err(|_| format!("delete_text: pos must be non-negative, got {pos}"))?;
        let len = usize::try_from(len)
            .map_err(|_| format!("delete_text: len must be non-negative, got {len}"))?;
        let (doc_path, backend) = self.find_doc_for_block(id).await?;
        backend
            .delete_text(id, pos, len)
            .await
            .map_err(|e| format!("delete_text: {e}"))?;
        self.save_doc(&doc_path).await?;
        Ok(OperationResult::irreversible(vec![]))
    }
}

#[async_trait]
impl OperationProvider for LoroBlockOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        use crate::__operations_has_cache;
        use crate::core::datasource::{
            __operations_block_operations, __operations_crud_operations,
            __operations_mark_operations, __operations_task_operations,
            __operations_text_operations,
        };

        let entity_name = Block::entity_name();
        let short_name = Block::short_name().expect("Block must have short_name");
        let id_column = "id";

        // Use resolver function for task_operations to resolve enum_from annotations
        let mut ops = __operations_task_operations::task_operations_with_resolver(
            self,
            entity_name,
            short_name,
            entity_name,
            id_column,
        );

        // Add operations from other trait sources
        ops.extend(
            __operations_crud_operations::crud_operations(
                entity_name,
                short_name,
                entity_name,
                id_column,
            )
            .into_iter(),
        );
        ops.extend(
            __operations_block_operations::block_operations(
                entity_name,
                short_name,
                entity_name,
                id_column,
            )
            .into_iter(),
        );
        ops.extend(
            __operations_mark_operations::mark_operations(
                entity_name,
                short_name,
                entity_name,
                id_column,
            )
            .into_iter(),
        );
        ops.extend(
            __operations_text_operations::text_operations(
                entity_name,
                short_name,
                entity_name,
                id_column,
            )
            .into_iter(),
        );
        ops.extend(
            __operations_has_cache::has_cache(entity_name, short_name, entity_name, id_column)
                .into_iter(),
        );

        ops
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        use crate::__operations_has_cache;
        use crate::core::datasource::{
            __operations_block_operations, __operations_crud_operations,
            __operations_mark_operations, __operations_task_operations,
            __operations_text_operations,
        };

        tracing::debug!(
            "[LoroBlockOperations::execute_operation] entity={}, op={}",
            entity_name,
            op_name
        );

        if entity_name != "block" {
            return Err(format!("Expected entity_name 'block', got '{}'", entity_name).into());
        }

        // Try HasCache operations (clear_cache)
        tracing::debug!("[LoroBlockOperations::execute_operation] Trying HasCache operations");
        match __operations_has_cache::dispatch_operation::<_, Block>(self, op_name, &params).await {
            Ok(op) => {
                tracing::debug!("[LoroBlockOperations::execute_operation] HasCache matched!");
                return Ok(op);
            }
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    tracing::debug!(
                        "[LoroBlockOperations::execute_operation] HasCache error: {}",
                        err
                    );
                    return Err(err);
                }
            }
        }

        // Try CRUD operations
        tracing::debug!("[LoroBlockOperations::execute_operation] Trying CRUD operations");
        match __operations_crud_operations::dispatch_operation::<_, Block>(self, op_name, &params)
            .await
        {
            Ok(op) => {
                tracing::debug!("[LoroBlockOperations::execute_operation] CRUD matched!");
                return Ok(op);
            }
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    tracing::debug!(
                        "[LoroBlockOperations::execute_operation] CRUD error: {}",
                        err
                    );
                    return Err(err);
                }
            }
        }

        // Handle "update" operation (forwards to create which does upsert)
        if op_name == "update" {
            tracing::debug!("[LoroBlockOperations::execute_operation] Handling update operation");
            return self.update_block(params).await;
        }

        // Try block operations
        match __operations_block_operations::dispatch_operation::<_, Block>(self, op_name, &params)
            .await
        {
            Ok(op) => return Ok(op),
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    return Err(err);
                }
            }
        }

        // Try mark operations (apply_mark / remove_mark)
        match __operations_mark_operations::dispatch_operation::<_, Block>(self, op_name, &params)
            .await
        {
            Ok(op) => return Ok(op),
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    return Err(err);
                }
            }
        }

        // Try text operations (insert_text / delete_text)
        match __operations_text_operations::dispatch_operation::<_, Block>(self, op_name, &params)
            .await
        {
            Ok(op) => return Ok(op),
            Err(err) => {
                if !UnknownOperationError::is_unknown(err.as_ref()) {
                    return Err(err);
                }
            }
        }

        // Try task operations
        __operations_task_operations::dispatch_operation::<_, Block>(self, op_name, &params).await
    }
}

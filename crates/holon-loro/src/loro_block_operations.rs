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
use holon_api::{ContentType, EntityName, EntityUri, Operation, Value};

use crate::LoroDocumentStore;
use crate::loro_backend::LoroBackend;
use crate::shared_tree::SharedTreeStore;
use holon_api::ApiError;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::repository::CoreOperations;
use holon_api::repository::Traversal;
use holon_core::{
    BlockDataSourceHelpers, BlockMaintenanceHelpers, BlockOperations, BlockQueryHelpers,
    CompletionStateInfo, CrudOperations, DataSource, MarkOperations, OperationProvider,
    OperationRegistry, OperationResult, Result, TaskOperations, TextOperations,
    UnknownOperationError,
};

/// Generic operations on Loro blocks.
///
/// Implements standard operation traits, delegating to `LoroBackend`. Reads go
/// straight to the Loro tree (the source of truth per ADR 0005), so this
/// provider needs no Turso-fed `QueryableCache` and works in no-Turso sessions.
pub struct LoroBlockOperations {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    /// Registry of shared subtree docs. When present, the per-operation write
    /// backend can route writes for blocks that were pruned into a shared doc
    /// (mount-aware write routing). `None` in configs without share machinery
    /// (no-Turso sessions, wasm, iroh-sync off), where no subtree is ever shared.
    shared_trees: Option<Arc<dyn SharedTreeStore>>,
}

impl LoroBlockOperations {
    pub fn new(doc_store: Arc<RwLock<LoroDocumentStore>>) -> Self {
        Self {
            doc_store,
            shared_trees: None,
        }
    }

    /// Attach the shared-tree registry so per-operation write backends can
    /// route writes into shared subtree docs (see `LoroBackend::with_shared_trees`).
    pub fn with_shared_trees(mut self, store: Arc<dyn SharedTreeStore>) -> Self {
        self.shared_trees = Some(store);
        self
    }

    /// Get the shared doc store (same instance used for writes).
    pub fn shared_doc_store(&self) -> Arc<RwLock<LoroDocumentStore>> {
        self.doc_store.clone()
    }

    /// Get the global backend (single LoroDoc for all blocks).
    async fn get_backend(&self, _: &str) -> Result<LoroBackend> {
        let store = self.doc_store.read().await;
        let collab_doc = store
            .get_global_doc()
            .await
            .map_err(|e| format!("Failed to get global doc: {}", e))?;
        let mut backend = LoroBackend::from_document(collab_doc);
        if let Some(store) = &self.shared_trees {
            backend = backend.with_shared_trees(store.clone());
        }
        Ok(backend)
    }

    /// Find the backend containing a block (always the global backend).
    async fn find_doc_for_block(&self, _: &str) -> Result<(String, LoroBackend)> {
        let backend = self.get_backend("").await?;
        Ok((backend.doc_id().to_string(), backend))
    }

    /// Save the global document after modification.
    async fn save_doc(&self, _: &str) -> Result<()> {
        let store = self.doc_store.read().await;
        store.save_all().await?;
        Ok(())
    }

    /// Dismiss one advice lesson under an anchor (ADR 0021 suppression + ADR 0022).
    ///
    /// A dismiss gesture appends `lesson_id` to the anchor's `advice_suppressed` set.
    /// This is a **read-modify-write**: read the anchor's current set, append the
    /// lesson if absent (idempotent), and write the whole set back via the production
    /// writer [`LoroBackend::set_block_advice_suppressed`].
    ///
    /// NOTE: the production writer is a **whole-set REPLACE over one LWW meta key**, so
    /// two concurrent dismissals of *different* lessons on the same anchor can lose one
    /// (last-writer-wins on the whole array). Per-element suppression (an H3-properties
    /// nested-map, one LWW key per dismissed lesson) is deferred.
    async fn dismiss_advice(&self, params: &StorageEntity) -> Result<OperationResult> {
        let anchor_id = params
            .get("anchor_id")
            .and_then(|v| v.as_string())
            .ok_or("dismiss_advice: missing 'anchor_id' parameter")?;
        let lesson_id = params
            .get("lesson_id")
            .and_then(|v| v.as_string())
            .ok_or("dismiss_advice: missing 'lesson_id' parameter")?;
        let lesson_uri = EntityUri::parse_owned(lesson_id.to_string())
            .map_err(|e| format!("dismiss_advice: invalid 'lesson_id' URI {lesson_id:?}: {e}"))?;

        let (doc_path, backend) = self.find_doc_for_block(anchor_id).await?;
        // Fail loud on a missing anchor — never silently write a fresh set.
        let anchor = backend
            .get_block(anchor_id)
            .await
            .map_err(|e| format!("dismiss_advice: anchor block {anchor_id:?} not found: {e}"))?;

        let mut suppressed = anchor.advice_suppressed.clone();
        if !suppressed.contains(&lesson_uri) {
            suppressed.push(lesson_uri);
        }
        backend
            .set_block_advice_suppressed(anchor_id, &suppressed)
            .await
            .map_err(|e| format!("dismiss_advice: {e}"))?;
        self.save_doc(&doc_path).await?;
        Ok(OperationResult::irreversible(vec![]))
    }
}

#[async_trait]
impl DataSource<Block> for LoroBlockOperations {
    async fn get_all(&self) -> Result<Vec<Block>> {
        let backend = self.get_backend("").await?;
        Ok(backend.get_all_blocks(Traversal::ALL_BUT_ROOT).await?)
    }

    async fn get_by_id(&self, id: &str) -> Result<Option<Block>> {
        let backend = self.get_backend("").await?;
        match backend.get_block(id).await {
            Ok(block) => Ok(Some(block)),
            Err(ApiError::BlockNotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }
}

#[async_trait]
impl BlockQueryHelpers<Block> for LoroBlockOperations {
    async fn children_ordered(&self, parent_id: &EntityUri) -> Result<Vec<Block>> {
        // Read child order straight from the Loro tree — its fractional index is
        // the source of truth for sibling order (ADR 0005), even when Turso is
        // also wired (the Turso `sort_key` column is only a projection of it).
        // `list_children` returns child URIs in tree order; `get_blocks`
        // preserves that input order.
        let backend = self.get_backend("").await?;
        let child_ids = backend.list_children(parent_id.as_str()).await?;
        let blocks = backend.get_blocks(child_ids).await?;
        Ok(blocks)
    }
}
impl BlockMaintenanceHelpers<Block> for LoroBlockOperations {}
impl BlockDataSourceHelpers<Block> for LoroBlockOperations {}
impl BlockOperations<Block> for LoroBlockOperations {}

/// Build a `block` operation for an undo inverse (no display name needed —
/// inverses are executed by the undo stack, not surfaced as user actions).
fn block_op(op_name: &str, params: HashMap<String, Value>) -> Operation {
    Operation::new(EntityName::from("block"), op_name, "", params)
}

/// Hand-built descriptor for the `dismiss_advice` operation (ADR 0021/0022).
///
/// Params: `anchor_id` (the block the advice is woven under) and `lesson_id` (the
/// dismissed advice row). Both are block ids. The frontend emits this op with those
/// two params from a per-lesson dismiss affordance.
fn dismiss_advice_descriptor() -> OperationDescriptor {
    use holon_api::render_types::{OperationParam, TypeHint};
    let block = EntityName::from("block");
    OperationDescriptor {
        entity_name: block.clone(),
        entity_short_name: "block".to_string(),
        id_column: "id".to_string(),
        name: "dismiss_advice".to_string(),
        display_name: "Dismiss advice".to_string(),
        description: "Suppress this advice lesson under its anchor block".to_string(),
        required_params: vec![
            OperationParam {
                name: "anchor_id".to_string(),
                type_hint: TypeHint::EntityId {
                    entity_name: block.clone(),
                },
                description: "The anchor block the advice is woven under".to_string(),
            },
            OperationParam {
                name: "lesson_id".to_string(),
                type_hint: TypeHint::EntityId { entity_name: block },
                description: "The advice lesson block to dismiss".to_string(),
            },
        ],
        ..Default::default()
    }
}

#[async_trait]
impl CrudOperations<Block> for LoroBlockOperations {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
        let (doc_path, backend) = self.find_doc_for_block(id).await?;

        // Capture a provably-correct inverse for the plain-string content edit
        // (restore the prior text). Rich content (with marks), mark-only edits,
        // and property writes stay irreversible until their inverses exist.
        let undo: Option<Operation> = if field == "content" && matches!(value, Value::String(_)) {
            let prior = backend
                .get_block(id)
                .await
                .map_err(|e| format!("set_field('content'): capture prior content: {e}"))?;
            let mut params = HashMap::new();
            params.insert("id".to_string(), Value::String(id.to_string()));
            params.insert("field".to_string(), Value::String("content".to_string()));
            params.insert("value".to_string(), Value::String(prior.content));
            Some(block_op("set_field", params))
        } else {
            None
        };

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
                        // `obj` is a Value::Object payload (set_field caller serialized
                        // marks as a JSON string), not a CDC row from the jsonb column.
                        // ALLOW(jsonb_as_string): payload field, not CDC row.
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
            "sort_key" => {
                // Sibling order is owned by `place()`/`tree.mov_after` and
                // projected to SQL from the Loro fractional index — a block's
                // `sort_key` is never written through `set_field`. Routing it to
                // the meta `properties` map would silently drop it: the
                // Loro→SQL projector derives `sort_key` from
                // `tree.fractional_index(node)` and ignores properties. Mirror
                // `BlockCellRegistry::write_field`: fail loud so a positional
                // write surfaces as a bug instead of vanishing.
                return Err(format!(
                    "set_field(\"sort_key\") is unsupported on LoroBlockOperations: order is \
                     owned by place()/mov_after and projected from the fractional index; a \
                     set_field(\"sort_key\") reached the Loro CRUD provider for {id} — bug"
                )
                .into());
            }
            "parent_id" => {
                // Reparenting is a structural tree move, not a meta property.
                // The projector reads the parent from the tree (not properties),
                // so a property write would be silently lost. Route to the
                // backend's `tree.mov`, mirroring `BlockCellRegistry::write_field`.
                let new_parent = value.as_string().map(String::from).ok_or_else(|| {
                    format!("set_field(\"parent_id\"): expected String, got {value:?}")
                })?;
                backend
                    .update_parent_id(id, new_parent)
                    .await
                    .map_err(|e| format!("set_field(\"parent_id\") for {id}: {e}"))?;
            }
            _ => {
                // Store in properties. A bare `task_state` keyword write gets
                // its `task_state_category` sidecar derived and written in the
                // SAME commit — the pair invariant `Block::set_task_state`
                // establishes at the org parse boundary (see
                // `TaskState::category_str_for_keyword`); without this every
                // UI cycle dropped/staled the category.
                let mut props = HashMap::new();
                if field == "task_state" {
                    let category = match &value {
                        Value::Null => Value::Null,
                        Value::String(kw) => Value::String(
                            holon_api::TaskState::category_str_for_keyword(kw).to_string(),
                        ),
                        other => {
                            return Err(format!(
                                "set_field('task_state'): expected String or Null, got {other:?}"
                            )
                            .into());
                        }
                    };
                    props.insert("task_state_category".to_string(), category);
                }
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
        Ok(undo.map_or_else(
            || OperationResult::irreversible(vec![]),
            |op| OperationResult::new(vec![], op),
        ))
    }

    async fn create(&self, fields: holon_api::StorageEntity) -> Result<(String, OperationResult)> {
        // parent_id is required - it's either a document URI or a block ID
        let parent_id = fields
            .get("parent_id")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string())
            .ok_or("parent_id is required for block creation")?;

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

        // Only a genuine create has a clean inverse (delete the new block); an
        // upsert that updates an existing block is left irreversible.
        let is_new = existing_block.is_none();

        let block = if let Some(existing) = existing_block {
            // Block exists - update it instead of creating
            tracing::debug!(
                "[LoroBlockOperations::create] Block {} exists, updating instead",
                existing.id
            );

            // If parent changed, move the block in the tree
            // ALLOW(entity_uri_from_raw): parent_id from operation params dict
            let new_parent_ref = holon_api::EntityUri::from_raw(&parent_id);
            if existing.parent_id != new_parent_ref {
                backend
                    .move_block(&existing.id, new_parent_ref.clone(), None)
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
            // ALLOW(entity_uri_from_raw): parent_id from operation params dict
            let parent_uri = holon_api::EntityUri::from_raw(&parent_id);
            // ALLOW(entity_uri_from_raw): block_id from operation params 'id' field
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
            if !handled_fields.contains(&key.as_ref()) {
                props.insert(key.to_string(), value.clone());
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

        let result = if is_new {
            let mut params = HashMap::new();
            params.insert(
                "id".to_string(),
                Value::String(block_with_props.id.to_string()),
            );
            OperationResult::new(vec![], block_op("delete", params))
        } else {
            OperationResult::irreversible(vec![])
        };

        Ok((block_with_props.id.to_string(), result))
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
    async fn update_block(&self, fields: holon_api::StorageEntity) -> Result<OperationResult> {
        let (_block_id, result) = self.create(fields).await?;
        Ok(result)
    }
}

#[async_trait]
impl TaskOperations<Block> for LoroBlockOperations {
    async fn set_title(&self, id: &str, title: &str) -> Result<OperationResult> {
        // Get current content, replace first line
        let backend = self.get_backend("").await?;
        let block = backend.get_block(id).await?;
        let body: String = block.content.lines().skip(1).collect::<Vec<_>>().join("\n");
        let new_content = if body.is_empty() {
            title.to_string()
        } else {
            format!("{}\n{}", title, body)
        };
        self.set_field(id, "content", Value::String(new_content))
            .await
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
        // The canonical task-state property key is `task_state` — the org parser
        // (`block_params`), the SQL provider's `cycle_task_state`, and `Block::task_state()`
        // all read/write `properties["task_state"]`. Writing `"TODO"` here stored a stray
        // property the cycle never read back, so `cycle_task_state` (read `task_state`, write
        // `TODO`) was a no-op in Loro mode — Cmd+Enter never advanced the keyword.
        // `set_field("task_state")` pairs the `task_state_category` sidecar in
        // the same commit (see its properties branch), so this delegate keeps
        // the pair invariant.
        self.set_field(id, "task_state", Value::String(state)).await
    }

    async fn cycle_task_state(&self, id: &str) -> Result<OperationResult> {
        let backend = self.get_backend("").await?;
        let block = backend.get_block(id).await?;
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
        use holon_core::{
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
        ops.extend(__operations_crud_operations::crud_operations(
            entity_name,
            short_name,
            entity_name,
            id_column,
        ));
        ops.extend(__operations_block_operations::block_operations(
            entity_name,
            short_name,
            entity_name,
            id_column,
        ));
        ops.extend(__operations_mark_operations::mark_operations(
            entity_name,
            short_name,
            entity_name,
            id_column,
        ));
        ops.extend(__operations_text_operations::text_operations(
            entity_name,
            short_name,
            entity_name,
            id_column,
        ));

        // Advice dismissal (ADR 0021/0022) — a bespoke read-modify-write append that
        // isn't one of the macro-generated CRUD/block/text traits, so its descriptor
        // is built by hand. The frontend binds this to a per-lesson dismiss affordance.
        ops.push(dismiss_advice_descriptor());

        ops
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        use holon_core::{
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

        // Intent boundary (Model.md invariant 3): `execute_operation` is the
        // provider surface intents arrive on (dispatcher, MCP, and
        // no-dispatcher configs that hold this provider directly). Parse the
        // `set_field` field into the closed `BlockWriteField` vocabulary —
        // order keys and storage-internal fields fail loud instead of being
        // silently discarded downstream. Internal callers (move_block's
        // depth write, the task-state convenience setters) call the
        // `CrudOperations::set_field` method directly and are unaffected.
        if op_name == "set_field" {
            let field = params
                .get("field")
                .and_then(|v| v.as_string())
                .ok_or("block set_field: missing 'field' parameter")?;
            holon_api::BlockWriteField::parse(field)
                .map_err(|e| format!("intent boundary: {e}"))?;
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
                    tracing::warn!(
                        "[LoroBlockOperations::execute_operation] CRUD op '{}' failed: {:#}",
                        op_name,
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

        // Advice dismissal (ADR 0021/0022): append-one RMW over the anchor's
        // `advice_suppressed` set. Not a macro-trait op — dispatched by hand here.
        if op_name == "dismiss_advice" {
            return self.dismiss_advice(&params).await;
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

#[cfg(test)]
mod advice_dismiss_tests {
    use super::*;
    use holon_api::block::BlockContent;
    use holon_api::repository::CoreOperations;
    use std::collections::HashMap;

    /// Build ops over a temp store with one anchor block `block:anchor` (empty
    /// `advice_suppressed`), returning the anchor id.
    async fn ops_with_anchor() -> (LoroBlockOperations, tempfile::TempDir, String) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(
            dir.path().to_path_buf(),
        )));
        let ops = LoroBlockOperations::new(store);
        let backend = ops.get_backend("").await.expect("backend");
        let root = backend.create_placeholder_root("root").await.expect("root");
        let root_uri = EntityUri::parse_owned(root).expect("root uri");
        let anchor = backend
            .create_block(
                root_uri,
                BlockContent::text("a task"),
                Some(EntityUri::block("anchor")),
            )
            .await
            .expect("anchor block");
        ops.save_doc("").await.expect("save");
        let anchor_id = anchor.id.to_string();
        (ops, dir, anchor_id)
    }

    fn dismiss_params(anchor_id: &str, lesson_id: &str) -> StorageEntity {
        let mut p: StorageEntity = HashMap::new();
        p.insert("anchor_id".into(), Value::String(anchor_id.into()));
        p.insert("lesson_id".into(), Value::String(lesson_id.into()));
        p
    }

    async fn read_suppressed(ops: &LoroBlockOperations, anchor_id: &str) -> Vec<String> {
        let backend = ops.get_backend("").await.expect("backend");
        backend
            .get_block(anchor_id)
            .await
            .expect("anchor")
            .advice_suppressed
            .iter()
            .map(|u| u.to_string())
            .collect()
    }

    #[tokio::test]
    async fn dismiss_appends_then_is_idempotent() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let entity = EntityName::new("block");

        ops.execute_operation(
            &entity,
            "dismiss_advice",
            dismiss_params(&anchor, "block:l1"),
        )
        .await
        .expect("first dismiss");
        assert_eq!(read_suppressed(&ops, &anchor).await, vec!["block:l1"]);

        // Second dismiss of the SAME lesson is a no-op (idempotent RMW).
        ops.execute_operation(
            &entity,
            "dismiss_advice",
            dismiss_params(&anchor, "block:l1"),
        )
        .await
        .expect("idempotent dismiss");
        assert_eq!(read_suppressed(&ops, &anchor).await, vec!["block:l1"]);
    }

    #[tokio::test]
    async fn dismiss_second_lesson_preserves_first() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let entity = EntityName::new("block");

        ops.execute_operation(
            &entity,
            "dismiss_advice",
            dismiss_params(&anchor, "block:l1"),
        )
        .await
        .expect("dismiss l1");
        // Append-one, not whole-set replace: the first dismissal survives.
        ops.execute_operation(
            &entity,
            "dismiss_advice",
            dismiss_params(&anchor, "block:l2"),
        )
        .await
        .expect("dismiss l2");
        let got = read_suppressed(&ops, &anchor).await;
        assert!(
            got.contains(&"block:l1".to_string()),
            "l1 preserved: {got:?}"
        );
        assert!(
            got.contains(&"block:l2".to_string()),
            "l2 appended: {got:?}"
        );
        assert_eq!(got.len(), 2);
    }

    #[tokio::test]
    async fn dismiss_missing_anchor_fails_loud() {
        let (ops, _dir, _anchor) = ops_with_anchor().await;
        let entity = EntityName::new("block");
        let err = ops
            .execute_operation(
                &entity,
                "dismiss_advice",
                dismiss_params("block:ghost", "block:l1"),
            )
            .await
            .expect_err("missing anchor must fail loud");
        assert!(err.to_string().contains("dismiss_advice"), "got: {err}");
    }

    #[test]
    fn dismiss_advice_descriptor_shape() {
        let d = dismiss_advice_descriptor();
        assert_eq!(d.name, "dismiss_advice");
        assert_eq!(d.entity_name, EntityName::new("block"));
        let names: Vec<&str> = d.required_params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["anchor_id", "lesson_id"]);
    }
}

#[cfg(test)]
mod intent_boundary_tests {
    use super::*;
    use std::collections::HashMap;

    fn ops_over_temp_store() -> (LoroBlockOperations, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(
            dir.path().to_path_buf(),
        )));
        (LoroBlockOperations::new(store), dir)
    }

    /// Model.md invariant 3 at the provider surface: a `set_field` intent
    /// carrying an order key is rejected before any CRUD dispatch — the
    /// guard fires even in configs where no `OperationDispatcher` sits in
    /// front of this provider (no-Turso sessions, wasm).
    #[tokio::test]
    async fn execute_operation_rejects_set_field_over_order_keys() {
        let (ops, _dir) = ops_over_temp_store();
        for field in ["sort_key", "after_block_id"] {
            let mut params: StorageEntity = HashMap::new();
            params.insert("id".into(), Value::String("block:a".into()));
            params.insert("field".into(), Value::String(field.into()));
            params.insert("value".into(), Value::String("A5".into()));
            let err = ops
                .execute_operation(&EntityName::new("block"), "set_field", params)
                .await
                .expect_err("set_field over an order key must be rejected at the boundary");
            assert!(
                err.to_string().contains("order key"),
                "rejection must name the invariant, got: {err}"
            );
        }
    }
}

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

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::ApiError;
use holon_api::ContentType;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::Operation;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::block::BlockContent;
use holon_api::repository::CoreOperations;
use holon_api::repository::Traversal;
use holon_core::BlockDataSourceHelpers;
use holon_core::BlockMaintenanceHelpers;
use holon_core::BlockOperations;
use holon_core::BlockQueryHelpers;
use holon_core::CompletionStateInfo;
use holon_core::CrudOperations;
use holon_core::DataSource;
use holon_core::FieldDelta;
use holon_core::MarkOperations;
use holon_core::OperationProvider;
use holon_core::OperationRegistry;
use holon_core::OperationResult;
use holon_core::Result;
use holon_core::TaskOperations;
use holon_core::TextOperations;
use holon_core::UnknownOperationError;
use tokio::sync::RwLock;

use crate::LoroDocumentStore;
use crate::loro_backend::LoroBackend;
use crate::shared_tree::SharedTreeStore;

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
    /// (no-Turso sessions, wasm, iroh-sync off), where no subtree is ever
    /// shared.
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
    /// route writes into shared subtree docs (see
    /// `LoroBackend::with_shared_trees`).
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

    /// Dismiss one advice lesson under an anchor (ADR 0021 suppression + ADR
    /// 0022).
    ///
    /// A dismiss gesture appends `lesson_id` to the anchor's
    /// `advice_suppressed` set. This is a **read-modify-write**: read the
    /// anchor's current set, append the lesson if absent (idempotent), and
    /// write the whole set back via the production
    /// writer [`LoroBackend::set_block_advice_suppressed`].
    ///
    /// NOTE: the production writer is a **whole-set REPLACE over one LWW meta
    /// key**, so two concurrent dismissals of *different* lessons on the
    /// same anchor can lose one (last-writer-wins on the whole array).
    /// Per-element suppression (an H3-properties nested-map, one LWW key
    /// per dismissed lesson) is deferred.
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

    /// Element-wise `add_tag`: append one `tag` to a block's `tags` set
    /// (idempotent, invertible). Mirrors the SqlOperationProvider arm — same
    /// `OperationResult` shape so both authorities behave identically.
    ///
    /// CRDT caveat (honest): the Loro tag set is a **whole-array LWW meta key**
    /// ([`LoroBackend::set_block_tags`]), so two concurrent `add_tag`s of
    /// DIFFERENT tags on the same block can lose one on merge (last-writer-wins
    /// over the array). Per-element tag keys (H3-properties nested map, one LWW
    /// key per tag) are deferred — the SQL junction's per-row PK has no such
    /// limitation.
    async fn add_tag(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or("add_tag: missing 'id' parameter")?;
        let tag = params
            .get("tag")
            .and_then(|v| v.as_string())
            .ok_or("add_tag: missing 'tag' parameter")?;

        let (doc_path, backend) = self.find_doc_for_block(id).await?;
        // Fail loud on a missing block — never silently write a fresh set.
        let block = backend
            .get_block(id)
            .await
            .map_err(|e| format!("add_tag: block {id:?} not found: {e}"))?;

        // No-pages-under-non-pages (interim ruling 2026-07-13): marking a block
        // Page turns it into a page, so its immediate parent must also be a page
        // (or be `no_parent` — seed/root pages stay legal). The immediate-parent
        // check suffices by induction: an existing tree already honours the rule.
        if tag == holon_api::PAGE_TAG {
            let parent_is_page = match block.parent_id.as_block_id() {
                Some(parent_id) => {
                    let parent = backend.get_block(parent_id).await.map_err(|e| {
                        format!("add_tag: parent block {parent_id:?} not found: {e}")
                    })?;
                    Some(parent.is_page())
                }
                None => None,
            };
            if holon_core::block_op_catalog::page_under_non_page_prohibited(true, parent_is_page) {
                return Err(holon_core::block_op_catalog::add_page_tag_rejection(
                    id,
                    block.parent_id.as_str(),
                )
                .into());
            }
        }

        let mut tags = block.tags.clone();
        let newly_added = tags.insert(tag);
        let inverse = block_op(
            "remove_tag",
            HashMap::from([
                ("id".to_string(), Value::String(id.to_string())),
                ("tag".to_string(), Value::String(tag.to_string())),
            ]),
        );
        if !newly_added {
            // Idempotent no-op: report a VACUOUS delta (old == new) so the
            // engine never journals an undo entry. Inverse-correctness: undoing
            // an idempotent re-add must not strip a tag that was already there.
            let changes = vec![FieldDelta::history_only(
                id,
                "tags",
                Value::String(tag.to_string()),
                Value::String(tag.to_string()),
            )];
            return Ok(OperationResult::new(changes, inverse));
        }
        backend
            .set_block_tags(id, &tags.to_vec())
            .await
            .map_err(|e| format!("add_tag: {e}"))?;
        self.save_doc(&doc_path).await?;
        // `tags` is a junction/meta field the column-only staleness reader
        // cannot fingerprint, so the delta is `history_only` — recorded in the
        // history relation but excluded from the undo precondition.
        let changes = vec![FieldDelta::history_only(
            id,
            "tags",
            Value::Null,
            Value::String(tag.to_string()),
        )];
        Ok(OperationResult::new(changes, inverse))
    }

    /// Element-wise `remove_tag`: drop one `tag` from a block's `tags` set
    /// (idempotent, invertible). Symmetric to [`Self::add_tag`].
    async fn remove_tag(&self, params: &StorageEntity) -> Result<OperationResult> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or("remove_tag: missing 'id' parameter")?;
        let tag = params
            .get("tag")
            .and_then(|v| v.as_string())
            .ok_or("remove_tag: missing 'tag' parameter")?;

        let (doc_path, backend) = self.find_doc_for_block(id).await?;
        let block = backend
            .get_block(id)
            .await
            .map_err(|e| format!("remove_tag: block {id:?} not found: {e}"))?;

        // Unmarking a Page whose direct children are themselves pages would
        // leave those children as pages under a non-page block. Reject loud
        // (cascade-unmark is a surprising bulk mutation — deliberately not done).
        if tag == holon_api::PAGE_TAG && block.is_page() {
            let child_ids = backend.list_children(id).await?;
            let children = backend.get_blocks(child_ids).await?;
            if children.iter().any(|c| c.is_page()) {
                return Err(holon_core::block_op_catalog::remove_page_tag_rejection(id).into());
            }
        }

        let mut tags = block.tags.clone();
        let was_present = tags.remove(tag);
        let inverse = block_op(
            "add_tag",
            HashMap::from([
                ("id".to_string(), Value::String(id.to_string())),
                ("tag".to_string(), Value::String(tag.to_string())),
            ]),
        );
        if !was_present {
            let changes = vec![FieldDelta::history_only(
                id,
                "tags",
                Value::Null,
                Value::Null,
            )];
            return Ok(OperationResult::new(changes, inverse));
        }
        backend
            .set_block_tags(id, &tags.to_vec())
            .await
            .map_err(|e| format!("remove_tag: {e}"))?;
        self.save_doc(&doc_path).await?;
        let changes = vec![FieldDelta::history_only(
            id,
            "tags",
            Value::String(tag.to_string()),
            Value::Null,
        )];
        Ok(OperationResult::new(changes, inverse))
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

/// Build the `set_field("content", Object{text, marks})` payload that restores
/// a block's rich content (text AND marks) as ONE atomic value. Routed through
/// the `content=Object` write path (`update_block_marked`), which clears every
/// mark key over the full range before re-applying — so restoring an
/// originally-plain block (empty `marks`) genuinely strips the marks a rich
/// write added, rather than leaving them pinned to surviving scalars (the
/// Peritext trap a text-only restore would fall into).
fn rich_content_restore_value(text: &str, marks: &[holon_api::MarkSpan]) -> Value {
    let mut obj = HashMap::new();
    obj.insert("text".to_string(), Value::String(text.to_string()));
    obj.insert(
        "marks".to_string(),
        Value::String(holon_api::marks_to_json(marks)),
    );
    Value::Object(obj)
}

/// Parse an edge-field write value (`Value::Array` of strings, or `Value::Null`
/// for the empty set) into owned strings. Fails loud on any non-string entry.
fn edge_string_targets(value: &Value, field: &str) -> std::result::Result<Vec<String>, String> {
    match value {
        Value::Array(items) => items
            .iter()
            .map(|v| {
                v.as_string().map(String::from).ok_or_else(|| {
                    format!("set_field('{field}'): edge target entry not a string: {v:?}")
                })
            })
            .collect(),
        Value::Null => Ok(Vec::new()),
        other => Err(format!(
            "set_field('{field}'): expected Array of strings, got {other:?}"
        )),
    }
}

#[async_trait]
impl CrudOperations<Block> for LoroBlockOperations {
    async fn set_field(&self, id: &str, field: &str, value: Value) -> Result<OperationResult> {
        let (doc_path, backend) = self.find_doc_for_block(id).await?;

        // Capture the prior block state ONCE, up front, so we can build both a
        // provably-correct inverse AND a staleness fingerprint (`changes`) for
        // the field being written. Two dogfood #4 data-integrity bugs traced
        // here:
        //   * `task_state` writes (from `cycle_task_state`) had NO inverse, so the op
        //     was `DeclaredIrreversible` → no undo entry was pushed → undo silently
        //     consumed an unrelated entry (a "no-op" cycle undo).
        //   * `content` writes returned EMPTY `changes`, so a `split_block` that
        //     delegates here inherited an empty precondition — disabling the
        //     stale-guard entirely and letting an undo-after-delete replay a stale
        //     inverse that destroyed unrelated blocks (P1 data loss).
        // Rich content (Object with marks) and mark-only edits stay irreversible.
        let prior = backend
            .get_block(id)
            .await
            .map_err(|e| format!("set_field('{field}'): capture prior state: {e}"))?;

        let (undo, changes): (Option<Operation>, Vec<FieldDelta>) = match field {
            "content" if matches!(value, Value::String(_)) => {
                let old = Value::String(prior.content.clone());
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.to_string()));
                params.insert("field".to_string(), Value::String("content".to_string()));
                params.insert("value".to_string(), old.clone());
                // `content` is a projected column the `SqlUndoStateReader` can
                // read, so a real fingerprint arms the stale-guard: an undo that
                // runs after the block was deleted/edited reads a divergent (or
                // absent) value and is dropped loudly instead of replaying a
                // stale inverse.
                (
                    Some(block_op("set_field", params)),
                    vec![FieldDelta::new(
                        id.to_string(),
                        "content",
                        old,
                        value.clone(),
                    )],
                )
            }
            "task_state" => {
                let old = match prior.get_property_str("task_state") {
                    Some(s) => Value::String(s),
                    None => Value::Null,
                };
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.to_string()));
                params.insert("field".to_string(), Value::String("task_state".to_string()));
                params.insert("value".to_string(), old);
                // `task_state` lives in the `properties` JSON blob, not a column,
                // so the column-only reader can't fingerprint it — empty
                // `changes` (single-writer safe). The inverse is still real, so
                // a cycle now pushes an undoable entry.
                (Some(block_op("set_field", params)), Vec::new())
            }
            f if holon_api::EdgeField::is_edge_column(f) => {
                // Edge fields are set-valued: a write is a whole-set replace, so
                // the inverse restores the PRIOR full set. The junction is not a
                // column the `SqlUndoStateReader` fingerprints, so `changes` is
                // empty (single-writer safe) while the inverse is still real.
                let edge = holon_api::EdgeField::ALL
                    .iter()
                    .copied()
                    .find(|e| e.column() == f)
                    .expect("is_edge_column ⇒ a matching EdgeField");
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.to_string()));
                params.insert("field".to_string(), Value::String(f.to_string()));
                params.insert("value".to_string(), edge.param_value(&prior));
                (Some(block_op("set_field", params)), Vec::new())
            }
            "content" if matches!(value, Value::Object(_)) => {
                // Rich content write (text + marks). The exact inverse restores
                // BOTH prior text and prior marks atomically as one `content`
                // Object value (see `rich_content_restore_value`). The `content`
                // FieldDelta arms the stale-guard ONLY when the text actually
                // changed: a marks-only edit under an Object write leaves the
                // projected `content` column untouched, so a real delta would be
                // vacuous (old == new) and the engine would refuse to journal the
                // entry. In that case fall back to empty `changes`
                // (single-writer-safe), matching the `marks` arm below.
                let prior_marks = prior.marks.clone().unwrap_or_default();
                let old = rich_content_restore_value(&prior.content, &prior_marks);
                let new_text = match &value {
                    Value::Object(obj) => obj
                        .get("text")
                        .and_then(|v| v.as_string())
                        .unwrap_or_default()
                        .to_string(),
                    _ => unreachable!("guarded by matches!(value, Value::Object(_))"),
                };
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.to_string()));
                params.insert("field".to_string(), Value::String("content".to_string()));
                params.insert("value".to_string(), old);
                let changes = if new_text != prior.content {
                    vec![FieldDelta::new(
                        id.to_string(),
                        "content",
                        Value::String(prior.content.clone()),
                        Value::String(new_text),
                    )]
                } else {
                    Vec::new()
                };
                (Some(block_op("set_field", params)), changes)
            }
            "marks" => {
                // Mark-only write: keep the text, replace the mark set. The exact
                // inverse restores prior (text, marks) atomically via the same
                // `content=Object` whole-set-restore path. The text never
                // changes, so the projected `content` column can't fingerprint a
                // marks-only edit — empty `changes` (single-writer-safe), while
                // the inverse is still a real, provably-correct restore.
                let prior_marks = prior.marks.clone().unwrap_or_default();
                let old = rich_content_restore_value(&prior.content, &prior_marks);
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.to_string()));
                params.insert("field".to_string(), Value::String("content".to_string()));
                params.insert("value".to_string(), old);
                (Some(block_op("set_field", params)), Vec::new())
            }
            _ => {
                // Every other field is routed to the block's properties map by
                // the `_` write arm below (DEADLINE/PRIORITY from
                // `set_due_date`/`set_priority`, and any generic org drawer
                // key). Capture the prior value for an EXACT inverse. A
                // previously-ABSENT property inverts to a REMOVE: `Value::Null`
                // routed back through the write arm's delete path, so undo of a
                // first-time property write leaves the block genuinely
                // property-free rather than pinning a null-valued key. The
                // properties blob is not a column the `SqlUndoStateReader`
                // fingerprints, so `changes` stays empty (single-writer safe)
                // while the inverse is a real, provably-correct restore.
                let old = prior.get_property(field).unwrap_or(Value::Null);
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.to_string()));
                params.insert("field".to_string(), Value::String(field.to_string()));
                params.insert("value".to_string(), old);
                (Some(block_op("set_field", params)), Vec::new())
            }
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
                        let marks_json = obj
                            .get("marks")
                            // ALLOW(jsonb_as_string): payload field, not CDC row.
                            .and_then(|v| v.as_string())
                            .ok_or_else(|| {
                                "set_field('content', Object): missing 'marks' JSON string \
                                     field"
                                    .to_string()
                            })?;
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
            f if holon_api::EdgeField::is_edge_column(f) => {
                // Set-valued edge field: write the tree node's dedicated meta key
                // so the Loro→SQL projector reads it into the matching junction
                // table. Routing it into `properties` (the `_` arm) would be
                // silently lost — the junction never sees it. Generic over every
                // `EdgeField` member (no per-field branch).
                let targets = edge_string_targets(&value, f)?;
                backend
                    .set_block_edge_field(id, f, &targets)
                    .await
                    .map_err(|e| format!("set_field('{f}') for {id}: {e:#}"))?;
            }
            _ => {
                // Store in the properties map. A `Value::Null` REMOVES the key
                // (the exact inverse of a previously-absent property) instead
                // of pinning a null blob — `update_block_fields` routes through
                // `apply_field_changes_to_meta`, which deletes on Null and
                // inserts otherwise, per-key (H3-safe), matching the
                // `LoroMetaCellBacking` write path.
                //
                // A bare `task_state` keyword write gets its
                // `task_state_category` sidecar derived and written in the SAME
                // commit — the pair invariant `Block::set_task_state`
                // establishes at the org parse boundary (see
                // `TaskState::category_str_for_keyword`); a `task_state` cleared
                // to Null removes both keys together.
                let mut fields: Vec<(String, Value, Value)> = Vec::new();
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
                    fields.push(("task_state_category".to_string(), Value::Null, category));
                }
                fields.push((field.to_string(), Value::Null, value));
                backend
                    .update_block_fields(id, &fields)
                    .await
                    .map_err(|e| format!("Failed to update property: {}", e))?;
            }
        }

        self.save_doc(&doc_path).await?;

        // Propagation to downstream consumers is handled by `LoroSyncController`
        // via `doc.subscribe_root`. `changes` is NOT a second write path (the
        // dispatcher ignores it) — it feeds the engine's precondition
        // fingerprint + history relation only.
        Ok(match undo {
            Some(op) => OperationResult::new(changes, op),
            None => OperationResult::irreversible(changes),
        })
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

        // `content` is normally a plain String. A `delete` inverse restores a
        // rich block as an `Object{text, marks}` payload (see `delete`) so the
        // marks come back atomically with the text: extract the text for
        // `BlockContent`, and remember the marks to re-apply via Peritext once
        // the node exists (`create_block_with_properties` writes only the raw
        // text).
        let (content, restore_marks): (String, Option<Vec<holon_api::MarkSpan>>) =
            match fields.get("content") {
                Some(Value::Object(obj)) => {
                    let text = obj
                        .get("text")
                        .and_then(|v| v.as_string())
                        .ok_or("create: content Object missing 'text' string field")?
                        .to_string();
                    let marks_json = obj
                        .get("marks")
                        // ALLOW(jsonb_as_string): payload field, not a CDC row.
                        .and_then(|v| v.as_string())
                        .ok_or("create: content Object missing 'marks' JSON string field")?;
                    let marks = holon_api::marks_from_json(marks_json)
                        .map_err(|e| format!("create: content marks JSON parse error: {e}"))?;
                    (text, Some(marks))
                }
                Some(Value::String(s)) => (s.clone(), None),
                None => (String::new(), None),
                Some(other) => {
                    return Err(format!("create: unsupported content shape {other:?}").into());
                }
            };

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

        let source_name = fields
            .get("source_name")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string());

        let block_id = fields
            .get("id")
            .and_then(|v| v.as_string())
            .map(|s| s.to_string());

        tracing::debug!(
            "[LoroBlockOperations::create] doc_id={:?}, block_id={:?}, parent_id={:?}, \
             content_type={:?}, source_language={:?}",
            doc_id,
            block_id,
            parent_id,
            content_type,
            source_language
        );

        // Edge fields (tags / requires / advice_suppressed) are set-valued
        // junctions the projector reads from dedicated Loro meta keys, NOT the
        // `properties` blob. A `delete` inverse restores them by passing them
        // here; absent (every normal create caller) ⇒ empty, unchanged
        // behaviour. Routing them into `properties` would silently orphan a
        // resurrected page (the `Page` tag makes a doc resolvable).
        let tags: holon_api::Tags = match fields.get("tags") {
            Some(v) => edge_string_targets(v, "tags")?.into_iter().collect(),
            None => holon_api::Tags::default(),
        };
        let requires: Vec<EntityUri> = match fields.get("requires") {
            Some(v) => edge_string_targets(v, "requires")?
                .iter()
                // ALLOW(entity_uri_from_raw): edge target from create op params dict
                .map(|s| EntityUri::from_raw(s))
                .collect(),
            None => Vec::new(),
        };
        let advice_suppressed: Vec<EntityUri> = match fields.get("advice_suppressed") {
            Some(v) => edge_string_targets(v, "advice_suppressed")?
                .iter()
                // ALLOW(entity_uri_from_raw): edge target from create op params dict
                .map(|s| EntityUri::from_raw(s))
                .collect(),
            None => Vec::new(),
        };

        // Build the appropriate BlockContent based on content_type. Image must
        // map to the dedicated variant — otherwise the block is stored in Loro
        // as `content_type = "text"` and the org `[[file:…]]` image classification
        // is lost on the create round-trip.
        let block_content = match content_type {
            ContentType::Source => {
                let lang = source_language.as_deref().unwrap_or("text");
                let mut sb = holon_api::block::SourceBlock::new(lang, content.clone());
                sb.name = source_name.clone();
                BlockContent::Source(sb)
            }
            ContentType::Image => BlockContent::image(content.clone()),
            ContentType::Text => BlockContent::text(content.clone()),
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
                .create_block_with_properties(
                    parent_uri,
                    block_content,
                    block_uri,
                    &HashMap::new(),
                    &tags,
                    &requires,
                    &advice_suppressed,
                )
                .await
                .map_err(|e| format!("Failed to create block: {}", e))?
        };

        // Set additional properties (excluding fields handled above and source block
        // fields)
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
            // Edge fields routed to their junctions above (not the props blob).
            "tags",
            "requires",
            "advice_suppressed",
            // Positional anchors (applied below). `after_block_id` is the canonical
            // key every prod caller uses; `after` is the delete-inverse restore key.
            // Both are operation-control metadata — stripped here so they NEVER land
            // in the persisted properties blob.
            holon_api::POSITION_AFTER_BLOCK_ID_PARAM,
            "after",
            // Flattened below, not stored verbatim.
            "properties",
        ];
        for (key, value) in &fields {
            if !handled_fields.contains(&key.as_ref()) {
                props.insert(key.to_string(), value.clone());
            }
        }
        // `properties` arrives as a JSON-object string. Flatten it into individual
        // keys, exactly like `SqlOperationProvider` — storing the raw string under a
        // literal `properties` key would diverge the Loro store from Turso on every
        // create-with-properties. Explicit per-key params win over the blob
        // (`or_insert`), matching Turso's merge order. Fail loud on a malformed blob.
        if let Some(props_val) = fields.get("properties") {
            let json = props_val.as_string().unwrap_or_else(|| {
                panic!("block.create 'properties' param must be a JSON string, got {props_val:?}")
            });
            let map: HashMap<String, Value> = serde_json::from_str(json).unwrap_or_else(|e| {
                panic!("block.create 'properties' is not a valid JSON object ({json:?}): {e}")
            });
            for (k, v) in map {
                props.entry(k).or_insert(v);
            }
        }
        if !props.is_empty() {
            backend
                .update_block_properties(block.id.as_str(), &props)
                .await
                .map_err(|e| format!("Failed to set properties: {}", e))?;
        }

        // Rich content restore (delete inverse): re-apply the captured marks via
        // Peritext over the just-written text. `create_block_with_properties`
        // wrote the raw text only, so a marked block would otherwise resurrect
        // plain — lossy. `update_block_marked` rewrites text AND marks together.
        if let Some(marks) = &restore_marks {
            backend
                .update_block_marked(block.id.as_str(), &content, marks)
                .await
                .map_err(|e| format!("create: restore marks for {}: {e}", block.id))?;
        }

        // Positional placement. One canonical primitive serves two callers:
        //   * `after_block_id` (`POSITION_AFTER_BLOCK_ID_PARAM`) — the
        //     positional-create key every prod caller uses; places a freshly created
        //     block immediately after its predecessor sibling in one op.
        //   * `after` — the delete-inverse restore key, restoring a resurrected block
        //     to its original sibling slot.
        // Both carry identical value semantics: a String predecessor sibling id, or
        // `Null` for "first child". Absent ⇒ leave it where `create` put it (append).
        // Loro owns order via the fractional index, so `update_block_position` is the
        // single primitive.
        let parse_anchor = |key: &str, v: &Value| -> Result<Option<String>> {
            match v {
                Value::String(p) => Ok(Some(p.clone())),
                Value::Null => Ok(None),
                other => {
                    Err(format!("create: '{key}' must be String or Null, got {other:?}").into())
                }
            }
        };
        let canonical = fields
            .get(holon_api::POSITION_AFTER_BLOCK_ID_PARAM)
            .map(|v| parse_anchor(holon_api::POSITION_AFTER_BLOCK_ID_PARAM, v))
            .transpose()?;
        let restore = fields
            .get("after")
            .map(|v| parse_anchor("after", v))
            .transpose()?;
        // Fail loud when both keys arrive disagreeing — an illegal, ambiguous
        // op we refuse rather than silently pick a winner.
        if let (Some(c), Some(r)) = (&canonical, &restore)
            && c != r
        {
            return Err(format!(
                "create: conflicting positional anchors — after_block_id={c:?} but after={r:?}"
            )
            .into());
        }
        if let Some(predecessor) = canonical.or(restore) {
            backend
                .update_block_position(block.id.as_str(), &parent_id, predecessor.as_deref())
                .await
                .map_err(|e| format!("create: set position for {}: {e}", block.id))?;
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
            // Emit the create as one `id` field delta (Null → minted id), exactly
            // as the SqlOperationProvider create path does. This is what the
            // engine's `record_history` chokepoint consumes to append a
            // `block_history` op_group — without a delta the batch is empty and a
            // Loro-consolidator create silently records NO history, so the C2
            // `inv-history-records-all-creates` op-group floor is missed. The
            // delta is non-vacuous (old != new), so undo journaling is unchanged.
            OperationResult::new(
                vec![FieldDelta::new(
                    block_with_props.id.to_string(),
                    "id",
                    Value::Null,
                    Value::String(block_with_props.id.to_string()),
                )],
                block_op("delete", params),
            )
        } else {
            OperationResult::irreversible(vec![])
        };

        Ok((block_with_props.id.to_string(), result))
    }

    async fn delete(&self, id: &str) -> Result<OperationResult> {
        let (doc_path, backend) = self.find_doc_for_block(id).await?;

        // Capture the FULL block state BEFORE deleting so a LEAF delete is
        // identity-invertible: the inverse is a `create` with the SAME id,
        // parent_id, content+marks, tags/edges, and properties, restored at the
        // SAME sibling position (ADR 0024 — preserve identity, never
        // delete+recreate).
        //
        // Fail-closed on NON-LEAF (destructive-delete ruling 2026-07-21): a bare
        // `delete` NEVER cascades a subtree away silently. When the target has
        // children the caller must opt in explicitly via `delete_subtree` (drop
        // the whole subtree) or `delete_keep_children` (reparent the children
        // first). This is the loud, fail-closed backstop that also protects the
        // MCP/agent path — no caller can cascade by accident.
        let block = match backend.get_block(id).await {
            Ok(block) => block,
            // Absent (never seeded / already deleted): `delete_block` is an
            // idempotent no-op and there is nothing to resurrect.
            Err(ApiError::BlockNotFound { .. }) => {
                return Ok(OperationResult::declared_irreversible(
                    vec![],
                    "delete: target block absent (nothing to resurrect)",
                ));
            }
            Err(e) => return Err(format!("delete: capture block {id}: {e}").into()),
        };

        let children = backend
            .list_children(id)
            .await
            .map_err(|e| format!("delete: list children of {id}: {e}"))?;
        if !children.is_empty() {
            return Err(format!(
                "delete: block {id} has {} child(ren); refusing to cascade. Use \
                 `delete_subtree` to delete the whole subtree, or \
                 `delete_keep_children` to reparent the children first.",
                children.len()
            )
            .into());
        }

        // Leaf: read the pre-delete sibling predecessor for the exact inverse.
        let siblings = backend
            .list_children(block.parent_id.as_str())
            .await
            .map_err(|e| format!("delete: list siblings under {}: {e}", block.parent_id))?;
        // ALLOW(entity_uri_from_raw): sibling ids read back from the Loro tree
        let target = EntityUri::from_raw(id);
        let idx = siblings
            .iter()
            // ALLOW(entity_uri_from_raw): sibling ids read back from the Loro tree
            .position(|s| EntityUri::from_raw(s) == target)
            .ok_or_else(|| {
                format!(
                    "delete: block {id} not among parent {} children",
                    block.parent_id
                )
            })?;
        let predecessor = if idx == 0 {
            None
        } else {
            Some(siblings[idx - 1].clone())
        };
        let create_op = block_op(
            "create",
            Self::delete_inverse_create_params(&block, predecessor),
        );

        backend
            .delete_block(id)
            .await
            .map_err(|e| format!("Failed to delete block: {}", e))?;

        self.save_doc(&doc_path).await?;

        // Leaf: exact create-inverse. Forward fingerprint mirrors the SqlOnly
        // authority — the `id` field is present pre-delete and absent after, so
        // an undo (`create`) drops loudly if the id was resurrected under it
        // before the undo ran.
        Ok(OperationResult::new(
            vec![FieldDelta::new(
                id.to_string(),
                "id",
                Value::String(id.to_string()),
                Value::Null,
            )],
            create_op,
        ))
    }
}

impl LoroBlockOperations {
    /// Build the `create`-op params that resurrect a just-deleted LEAF block
    /// identically: same id, parent, content (rich `Object{text, marks}` when
    /// the block carried marks, else plain text), edge fields, every stored
    /// property (task_state + its category sidecar, DEADLINE/PRIORITY, …), and
    /// an `after` positional anchor (predecessor sibling id, or `Null` for
    /// "first child") so `create` restores it at its original slot.
    fn delete_inverse_create_params(
        block: &Block,
        predecessor: Option<String>,
    ) -> HashMap<String, Value> {
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(block.id.to_string()));
        params.insert(
            "parent_id".into(),
            Value::String(block.parent_id.to_string()),
        );
        params.insert(
            "content_type".into(),
            Value::String(block.content_type.to_string()),
        );
        // Rich block ⇒ atomic Object payload so `create` reapplies marks over
        // the same text; plain block ⇒ bare string.
        let content = match &block.marks {
            Some(marks) => rich_content_restore_value(&block.content, marks),
            None => Value::String(block.content.clone()),
        };
        params.insert("content".into(), content);
        if let Some(lang) = &block.source_language {
            params.insert("source_language".into(), Value::String(lang.to_string()));
        }
        if let Some(name) = &block.source_name {
            params.insert("source_name".into(), Value::String(name.clone()));
        }
        if !block.tags.is_empty() {
            params.insert(
                "tags".into(),
                Value::Array(block.tags.to_vec().into_iter().map(Value::String).collect()),
            );
        }
        if !block.requires.is_empty() {
            params.insert(
                "requires".into(),
                Value::Array(
                    block
                        .requires
                        .iter()
                        .map(|u| Value::String(u.to_string()))
                        .collect(),
                ),
            );
        }
        if !block.advice_suppressed.is_empty() {
            params.insert(
                "advice_suppressed".into(),
                Value::Array(
                    block
                        .advice_suppressed
                        .iter()
                        .map(|u| Value::String(u.to_string()))
                        .collect(),
                ),
            );
        }
        for (key, value) in &block.properties {
            params.insert(key.clone(), value.clone());
        }
        params.insert(
            "after".into(),
            match predecessor {
                Some(pred) => Value::String(pred),
                None => Value::Null,
            },
        );
        params
    }

    /// Update a block with the given fields.
    ///
    /// Forwards to `create` which does upsert (create if not exists, update if
    /// exists).
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
        // (`block_params`), the SQL provider's `cycle_task_state`, and
        // `Block::task_state()` all read/write `properties["task_state"]`.
        // Writing `"TODO"` here stored a stray property the cycle never read
        // back, so `cycle_task_state` (read `task_state`, write `TODO`) was a
        // no-op in Loro mode — Cmd+Enter never advanced the keyword.
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
        // Capture the EXACT prior rich content (text + FULL mark set) BEFORE the
        // mark write. The inverse restores it atomically via the whole-set
        // `content=Object` path (`update_block_marked`), which clears every mark
        // over the whole range before re-applying — so undo restores the exact
        // prior marks. A blind `remove_mark(range, key)` inverse would instead
        // strip a PRE-EXISTING overlapping mark of the same key the user never
        // touched (e.g. applying Bold over a sub-range of an existing Bold span,
        // then undoing, must NOT punch a hole in the original span). Text is
        // unchanged by a mark op, so `changes` stays empty (single-writer safe),
        // matching the `set_field("marks")` arm.
        let prior = backend
            .get_block(id)
            .await
            .map_err(|e| format!("apply_mark: capture prior marks: {e}"))?;
        let prior_marks = prior.marks.clone().unwrap_or_default();
        backend
            .apply_inline_mark(id, start..end, &mark)
            .await
            .map_err(|e| format!("apply_inline_mark: {e}"))?;
        self.save_doc(&doc_path).await?;
        let inverse = block_op(
            "set_field",
            HashMap::from([
                ("id".to_string(), Value::String(id.to_string())),
                ("field".to_string(), Value::String("content".to_string())),
                (
                    "value".to_string(),
                    rich_content_restore_value(&prior.content, &prior_marks),
                ),
            ]),
        );
        Ok(OperationResult::new(vec![], inverse))
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
        // Capture the EXACT prior rich content (text + FULL mark set) BEFORE the
        // removal so the inverse restores the captured prior mark set for the
        // affected range — atomically via the whole-set `content=Object`
        // restore path. Symmetric to `apply_mark`; text is untouched so
        // `changes` stays empty (single-writer safe).
        let prior = backend
            .get_block(id)
            .await
            .map_err(|e| format!("remove_mark: capture prior marks: {e}"))?;
        let prior_marks = prior.marks.clone().unwrap_or_default();
        backend
            .remove_inline_mark(id, start..end, &key)
            .await
            .map_err(|e| format!("remove_inline_mark: {e}"))?;
        self.save_doc(&doc_path).await?;
        let inverse = block_op(
            "set_field",
            HashMap::from([
                ("id".to_string(), Value::String(id.to_string())),
                ("field".to_string(), Value::String("content".to_string())),
                (
                    "value".to_string(),
                    rich_content_restore_value(&prior.content, &prior_marks),
                ),
            ]),
        );
        Ok(OperationResult::new(vec![], inverse))
    }
}

#[async_trait]
impl TextOperations<Block> for LoroBlockOperations {
    async fn insert_text(&self, id: &str, pos: i64, text: String) -> Result<OperationResult> {
        let pos_usize = usize::try_from(pos)
            .map_err(|_| format!("insert_text: pos must be non-negative, got {pos}"))?;
        let (doc_path, backend) = self.find_doc_for_block(id).await?;
        // Capture prior content so the projected `content` column can be
        // fingerprinted (arms the undo stale-guard, same as set_field("content")).
        let old_content = backend
            .get_block(id)
            .await
            .map_err(|e| format!("insert_text: capture prior content: {e}"))?
            .content;
        backend
            .insert_text(id, pos_usize, &text)
            .await
            .map_err(|e| format!("insert_text: {e}"))?;
        self.save_doc(&doc_path).await?;
        let new_content = backend
            .get_block(id)
            .await
            .map_err(|e| format!("insert_text: read post-insert content: {e}"))?
            .content;
        // Exact inverse: delete the range just inserted. LoroText positions are
        // Unicode scalars, so the deleted length is the scalar count of `text` —
        // the same unit `delete_text`'s `len` expects.
        let inverse = block_op(
            "delete_text",
            HashMap::from([
                ("id".to_string(), Value::String(id.to_string())),
                ("pos".to_string(), Value::Integer(pos)),
                (
                    "len".to_string(),
                    Value::Integer(text.chars().count() as i64),
                ),
            ]),
        );
        Ok(OperationResult::new(
            vec![FieldDelta::new(
                id.to_string(),
                "content",
                Value::String(old_content),
                Value::String(new_content),
            )],
            inverse,
        ))
    }

    async fn delete_text(&self, id: &str, pos: i64, len: i64) -> Result<OperationResult> {
        let pos_usize = usize::try_from(pos)
            .map_err(|_| format!("delete_text: pos must be non-negative, got {pos}"))?;
        let len_usize = usize::try_from(len)
            .map_err(|_| format!("delete_text: len must be non-negative, got {len}"))?;
        let (doc_path, backend) = self.find_doc_for_block(id).await?;
        // Capture the exact substring about to be deleted (Unicode-scalar range
        // [pos, pos+len)) so the inverse can re-insert it verbatim. Char-indexed
        // to match LoroText's scalar positions.
        let old_content = backend
            .get_block(id)
            .await
            .map_err(|e| format!("delete_text: capture prior content: {e}"))?
            .content;
        let deleted: String = old_content
            .chars()
            .skip(pos_usize)
            .take(len_usize)
            .collect();
        backend
            .delete_text(id, pos_usize, len_usize)
            .await
            .map_err(|e| format!("delete_text: {e}"))?;
        self.save_doc(&doc_path).await?;
        let new_content = backend
            .get_block(id)
            .await
            .map_err(|e| format!("delete_text: read post-delete content: {e}"))?
            .content;
        // Exact inverse: re-insert the captured substring at the same position.
        let inverse = block_op(
            "insert_text",
            HashMap::from([
                ("id".to_string(), Value::String(id.to_string())),
                ("pos".to_string(), Value::Integer(pos)),
                ("text".to_string(), Value::String(deleted)),
            ]),
        );
        Ok(OperationResult::new(
            vec![FieldDelta::new(
                id.to_string(),
                "content",
                Value::String(old_content),
                Value::String(new_content),
            )],
            inverse,
        ))
    }
}

#[async_trait]
impl OperationProvider for LoroBlockOperations {
    fn operations(&self) -> Vec<OperationDescriptor> {
        use holon_core::__operations_block_operations;
        use holon_core::__operations_crud_operations;
        use holon_core::__operations_mark_operations;
        use holon_core::__operations_task_operations;
        use holon_core::__operations_text_operations;

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

        // Advice dismissal (ADR 0021/0022) — a bespoke read-modify-write append
        // that isn't one of the macro-generated CRUD/block/text traits. Its
        // descriptor is the ONE hand-built op shared by both write authorities,
        // so it lives in the shared catalog (BugFunnel row 26 was its drift).
        ops.push(holon_core::block_op_catalog::dismiss_advice_descriptor(
            &EntityName::from(entity_name),
            short_name,
        ));

        // Element-wise tag ops (idempotent, invertible) — bespoke shared
        // descriptors, single-sourced from the catalog like dismiss_advice.
        ops.push(holon_core::block_op_catalog::add_tag_descriptor(
            &EntityName::from(entity_name),
            short_name,
        ));
        ops.push(holon_core::block_op_catalog::remove_tag_descriptor(
            &EntityName::from(entity_name),
            short_name,
        ));

        ops
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        use holon_core::__operations_block_operations;
        use holon_core::__operations_crud_operations;
        use holon_core::__operations_mark_operations;
        use holon_core::__operations_task_operations;
        use holon_core::__operations_text_operations;

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

        // Element-wise tag mutation (idempotent, invertible) — bespoke
        // hand-dispatched ops, like dismiss_advice, not macro-trait ops.
        if op_name == "add_tag" {
            return self.add_tag(&params).await;
        }
        if op_name == "remove_tag" {
            return self.remove_tag(&params).await;
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
    use std::collections::HashMap;

    use holon_api::block::BlockContent;
    use holon_api::repository::CoreOperations;

    use super::*;

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

    /// Edge-field CRUD (composed-keystone `SetEdgeField{Tags/Requires}` RED,
    /// 2026-07-13): a `set_field` on a set-valued edge field must (a) land in
    /// the tree node's DEDICATED meta key so `get_block` reads it into the
    /// junction (NOT the `properties` blob, where the projector never sees
    /// it), and (b) carry a whole-set-restore inverse capturing the PRIOR
    /// full set.
    #[tokio::test]
    async fn edge_field_set_field_hits_junction_meta_and_inverts() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        // requires: establish a prior set {dep1}.
        ops.set_field(
            &anchor,
            "requires",
            Value::Array(vec![Value::String("block:dep1".into())]),
        )
        .await
        .expect("set requires");
        let read = backend.get_block(&anchor).await.expect("read");
        assert_eq!(
            read.requires
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>(),
            vec!["block:dep1".to_string()],
            "requires must land in the junction meta, read back by get_block"
        );

        // Whole-set replace to {dep1, dep2}; the inverse must restore {dep1}.
        let r2 = ops
            .set_field(
                &anchor,
                "requires",
                Value::Array(vec![
                    Value::String("block:dep1".into()),
                    Value::String("block:dep2".into()),
                ]),
            )
            .await
            .expect("set requires 2");
        let inverse = match r2.undo {
            holon_core::UndoAction::Undo(op) => op,
            other => panic!("edge set_field must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "set_field");
        assert_eq!(
            inverse.params.get("field"),
            Some(&Value::String("requires".into()))
        );
        assert_eq!(
            inverse.params.get("value"),
            Some(&Value::Array(vec![Value::String("block:dep1".into())])),
            "inverse must restore the PRIOR full set (whole-set restore, not element diff)"
        );

        // Applying the inverse restores {dep1}.
        ops.set_field(
            &anchor,
            "requires",
            inverse.params.get("value").unwrap().clone(),
        )
        .await
        .expect("apply inverse");
        let read3 = backend.get_block(&anchor).await.expect("read3");
        assert_eq!(
            read3
                .requires
                .iter()
                .map(|u| u.to_string())
                .collect::<Vec<_>>(),
            vec!["block:dep1".to_string()],
            "undo (inverse replay) restores the prior requires set"
        );

        // tags travel the same generic path (no per-field branch).
        ops.set_field(
            &anchor,
            "tags",
            Value::Array(vec![Value::String("task".into())]),
        )
        .await
        .expect("set tags");
        let read_tags = backend.get_block(&anchor).await.expect("read tags");
        assert_eq!(
            read_tags.tags.to_vec(),
            vec!["task".to_string()],
            "tags must land in the junction meta"
        );
    }

    /// Regression (keystone `inv-blocks-match-ref/org` RED, 2026-07-12): the
    /// org-ingest → Loro create path (`LoroBlockOperations::create`) receives
    /// `content_type = "image"` in its params (org `[[file:…]]`
    /// classification). It must store and read the block back as
    /// `ContentType::Image`. Before the `BlockContent::Image` variant it
    /// built `BlockContent::text(...)` here, so the block was persisted as
    /// `content_type = "text"` and the image classification was lost on the
    /// org round-trip.
    #[tokio::test]
    async fn image_content_type_survives_ingest_create() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(
            dir.path().to_path_buf(),
        )));
        let ops = LoroBlockOperations::new(store);
        let backend = ops.get_backend("").await.expect("backend");
        let root = backend.create_placeholder_root("root").await.expect("root");

        let mut fields: StorageEntity = HashMap::new();
        fields.insert("id".into(), Value::String("block:h1::img::0".into()));
        fields.insert("parent_id".into(), Value::String(root.clone()));
        fields.insert(
            "content".into(),
            Value::String("attachments/foo.png".into()),
        );
        fields.insert("content_type".into(), Value::String("image".into()));

        let (block_id, _) = ops.create(fields).await.expect("ingest create");

        let read = backend.get_block(&block_id).await.expect("read back");
        assert_eq!(
            read.content_type,
            ContentType::Image,
            "org-ingested image must persist as Image (was collapsing to Text), got {:?}",
            read.content_type
        );
        assert_eq!(read.content, "attachments/foo.png");
    }

    use holon_core::traits::UndoAction;

    /// Regression (BugFunnel dogfood #4, cycle-undo no-op): in Loro mode
    /// `set_field` only built an inverse for `content`, so a `task_state` write
    /// (the substrate of `cycle_task_state`) returned `DeclaredIrreversible` —
    /// no undo entry was pushed and pressing undo silently consumed an
    /// unrelated entry. A cycle must now be reversible with an inverse that
    /// targets `task_state` (NOT `content`).
    #[tokio::test]
    async fn cycle_task_state_is_reversible_and_targets_task_state() {
        let (ops, _dir, anchor) = ops_with_anchor().await;

        let result = ops.cycle_task_state(&anchor).await.expect("cycle");
        match &result.undo {
            UndoAction::Undo(op) => {
                assert_eq!(op.op_name, "set_field");
                assert_eq!(
                    op.params.get("field").and_then(|v| v.as_string()),
                    Some("task_state"),
                    "cycle's inverse must restore task_state, not content — got {:?}",
                    op.params
                );
                // Prior task_state was absent → inverse restores Null (removal).
                assert!(matches!(op.params.get("value"), Some(Value::Null)));
            }
            other => panic!("cycle_task_state must be reversible, got {other:?}"),
        }
    }

    /// Regression (BugFunnel dogfood #4, P1 stale-guard gap): in Loro mode
    /// `set_field("content")` returned EMPTY `changes`, so `split_block` (which
    /// delegates here) inherited an empty precondition and the staleness guard
    /// never fired — letting an undo-after-delete replay a stale inverse that
    /// destroyed unrelated blocks. The content write must now emit a real
    /// FieldDelta so the engine can fingerprint (and re-verify) the projected
    /// `content` column.
    #[tokio::test]
    async fn content_set_field_emits_fingerprint_delta() {
        let (ops, _dir, anchor) = ops_with_anchor().await;

        let result = ops
            .set_field(&anchor, "content", Value::String("edited".into()))
            .await
            .expect("set_field content");

        assert_eq!(result.changes.len(), 1, "expected one content FieldDelta");
        let delta = &result.changes[0];
        assert_eq!(delta.entity_id, anchor);
        assert_eq!(delta.field, "content");
        assert_eq!(delta.old_value, Value::String("a task".into()));
        assert_eq!(delta.new_value, Value::String("edited".into()));
        assert!(
            matches!(&result.undo, UndoAction::Undo(op)
                if op.params.get("value").and_then(|v| v.as_string()) == Some("a task")),
            "inverse must restore the prior content"
        );
    }

    /// `insert_text` must be reversible with an exact `delete_text` inverse:
    /// the inserted range is `text.chars().count()` Unicode scalars at `pos`,
    /// and replaying the inverse restores the block byte-for-byte. Gates the
    /// composite-transform coverage requirement (incremental text edits inside
    /// a compound UndoEntry).
    #[tokio::test]
    async fn insert_text_is_reversible_with_exact_delete_inverse() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        // "a task" -> insert "XX" at scalar 2 -> "a XXtask".
        let result = ops
            .insert_text(&anchor, 2, "XX".into())
            .await
            .expect("insert_text");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").content,
            "a XXtask"
        );

        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("insert_text must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "delete_text");
        assert_eq!(inverse.params.get("pos"), Some(&Value::Integer(2)));
        assert_eq!(inverse.params.get("len"), Some(&Value::Integer(2)));

        // Replaying the inverse restores the original content.
        let inverse_params: StorageEntity = inverse
            .params
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        ops.execute_operation(&EntityName::new("block"), "delete_text", inverse_params)
            .await
            .expect("replay inverse");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").content,
            "a task"
        );
    }

    /// `delete_text` must be reversible with an exact `insert_text` inverse
    /// carrying the deleted substring verbatim.
    #[tokio::test]
    async fn delete_text_is_reversible_with_exact_insert_inverse() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        // "a task" -> delete 4 scalars at 2 ("task") -> "a ".
        let result = ops.delete_text(&anchor, 2, 4).await.expect("delete_text");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").content,
            "a "
        );

        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("delete_text must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "insert_text");
        assert_eq!(inverse.params.get("pos"), Some(&Value::Integer(2)));
        assert_eq!(
            inverse.params.get("text"),
            Some(&Value::String("task".into()))
        );

        let inverse_params: StorageEntity = inverse
            .params
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        ops.execute_operation(&EntityName::new("block"), "insert_text", inverse_params)
            .await
            .expect("replay inverse");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").content,
            "a task"
        );
    }

    /// Regression guard for the index unit of the text inverses: Loro's plain
    /// `insert`/`delete` count Unicode scalars, and the captured inverse uses
    /// `.chars()` (also scalars). A future switch to the `_utf8` byte-indexed
    /// Loro APIs would silently corrupt multi-byte content — this test fails
    /// in that world ("äöü😀" is 4 scalars but 10 UTF-8 bytes).
    #[tokio::test]
    async fn text_inverses_are_scalar_indexed_for_multibyte_content() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        let result = ops
            .insert_text(&anchor, 2, "äöü😀".into())
            .await
            .expect("insert_text");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").content,
            "a äöü😀task"
        );
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("insert_text must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "delete_text");
        assert_eq!(inverse.params.get("pos"), Some(&Value::Integer(2)));
        assert_eq!(inverse.params.get("len"), Some(&Value::Integer(4)));
        let inverse_params: StorageEntity = inverse
            .params
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        ops.execute_operation(&EntityName::new("block"), "delete_text", inverse_params)
            .await
            .expect("replay insert inverse");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").content,
            "a task"
        );

        // Delete across multi-byte content and replay its insert inverse.
        ops.insert_text(&anchor, 2, "äöü😀".into())
            .await
            .expect("re-insert");
        let result = ops.delete_text(&anchor, 3, 2).await.expect("delete_text");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").content,
            "a ä😀task"
        );
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("delete_text must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "insert_text");
        assert_eq!(inverse.params.get("pos"), Some(&Value::Integer(3)));
        assert_eq!(
            inverse.params.get("text"),
            Some(&Value::String("öü".into()))
        );
        let inverse_params: StorageEntity = inverse
            .params
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        ops.execute_operation(&EntityName::new("block"), "insert_text", inverse_params)
            .await
            .expect("replay delete inverse");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").content,
            "a äöü😀task"
        );
    }

    /// Build a `set_field("content", Object{text, marks})` value payload.
    fn object_content(text: &str, marks: &[holon_api::MarkSpan]) -> Value {
        let mut obj = HashMap::new();
        obj.insert("text".to_string(), Value::String(text.to_string()));
        obj.insert(
            "marks".to_string(),
            Value::String(holon_api::marks_to_json(marks)),
        );
        Value::Object(obj)
    }

    fn bold(start: usize, end: usize) -> holon_api::MarkSpan {
        holon_api::MarkSpan::new(start, end, holon_api::InlineMark::Bold)
    }

    async fn replay_inverse(ops: &LoroBlockOperations, inverse: &Operation) {
        let params: StorageEntity = inverse
            .params
            .clone()
            .into_iter()
            .map(|(k, v)| (k.into(), v))
            .collect();
        ops.execute_operation(&EntityName::new("block"), &inverse.op_name, params)
            .await
            .expect("replay inverse");
    }

    /// A rich `set_field("content", Object{text, marks})` write must carry an
    /// EXACT inverse: replaying it restores BOTH the prior text and the prior
    /// mark set byte-for-byte. A prior PLAIN block (marks `None`) must come
    /// back plain — the whole-set restore strips the marks the forward
    /// write added, never leaving a Peritext mark pinned to surviving text.
    #[tokio::test]
    async fn set_field_object_content_is_reversible_text_and_marks() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        // Prior state: plain "a task", marks None.
        let prior = backend.get_block(&anchor).await.expect("read");
        assert_eq!(prior.content, "a task");
        assert_eq!(prior.marks, None);

        // Rich write: change text AND add a Bold mark.
        let result = ops
            .set_field(
                &anchor,
                "content",
                object_content("bold text", &[bold(0, 4)]),
            )
            .await
            .expect("set_field content Object");

        let after = backend.get_block(&anchor).await.expect("read");
        assert_eq!(after.content, "bold text");
        assert_eq!(after.marks, Some(vec![bold(0, 4)]));

        // Inverse shape: set_field on `content`, an Object restoring prior text.
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("rich content write must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "set_field");
        assert_eq!(
            inverse.params.get("field").and_then(|v| v.as_string()),
            Some("content")
        );
        assert!(
            matches!(inverse.params.get("value"), Some(Value::Object(_))),
            "inverse value must be a rich Object payload"
        );

        // Replay ⇒ back to plain "a task" with marks None (byte + mark exact).
        replay_inverse(&ops, &inverse).await;
        let restored = backend.get_block(&anchor).await.expect("read");
        assert_eq!(restored.content, "a task");
        assert_eq!(restored.marks, None, "plain prior must restore plain");
    }

    /// A mark-only `set_field("marks", ...)` write must be reversible: the
    /// inverse restores the prior mark set (and text) exactly. Starting from an
    /// already-rich block, replacing its marks and undoing returns the original
    /// marks.
    #[tokio::test]
    async fn set_field_marks_only_is_reversible() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        // Make the block rich first: "a task" with Bold over [0,1).
        ops.set_field(&anchor, "content", object_content("a task", &[bold(0, 1)]))
            .await
            .expect("seed rich");
        assert_eq!(
            backend.get_block(&anchor).await.expect("read").marks,
            Some(vec![bold(0, 1)])
        );

        // Mark-only write: replace marks with Bold over [2,6) ("task").
        let new_marks = holon_api::marks_to_json(&[bold(2, 6)]);
        let result = ops
            .set_field(&anchor, "marks", Value::String(new_marks))
            .await
            .expect("set_field marks");
        let after = backend.get_block(&anchor).await.expect("read");
        assert_eq!(after.content, "a task", "marks-only write keeps text");
        assert_eq!(after.marks, Some(vec![bold(2, 6)]));

        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("marks-only write must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "set_field");
        assert_eq!(
            inverse.params.get("field").and_then(|v| v.as_string()),
            Some("content"),
            "inverse restores via the atomic content=Object path"
        );

        replay_inverse(&ops, &inverse).await;
        let restored = backend.get_block(&anchor).await.expect("read");
        assert_eq!(restored.content, "a task");
        assert_eq!(
            restored.marks,
            Some(vec![bold(0, 1)]),
            "undo restores the prior mark set exactly"
        );
    }

    /// Multibyte round-trip: a rich write over content with non-ASCII scalars
    /// (mark ranges are Unicode-scalar offsets) must restore byte-for-byte.
    #[tokio::test]
    async fn set_field_object_content_multibyte_roundtrip() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        // "äöü😀 tail" — Bold over the 4 leading multibyte scalars [0,4).
        let text = "äöü😀 tail";
        let result = ops
            .set_field(&anchor, "content", object_content(text, &[bold(0, 4)]))
            .await
            .expect("rich multibyte write");
        let after = backend.get_block(&anchor).await.expect("read");
        assert_eq!(after.content, text);
        assert_eq!(after.marks, Some(vec![bold(0, 4)]));

        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("must be reversible, got {other:?}"),
        };
        replay_inverse(&ops, &inverse).await;
        let restored = backend.get_block(&anchor).await.expect("read");
        assert_eq!(restored.content, "a task");
        assert_eq!(restored.marks, None);
    }

    /// Seed `id`/`text` as a child of `parent` (full URI), returning the
    /// child's full id. Used to build ordered sibling fixtures for
    /// delete-inverse tests.
    async fn seed_child(backend: &LoroBackend, parent: &str, id: &str, text: &str) -> String {
        let parent_uri = EntityUri::parse_owned(parent.to_string()).expect("parent uri");
        let block = backend
            .create_block(
                parent_uri,
                BlockContent::text(text),
                Some(EntityUri::block(id)),
            )
            .await
            .expect("seed child");
        block.id.to_string()
    }

    /// A LEAF `delete` under the default Loro authority must be EXACTLY
    /// reversible: undo restores a byte-identical block — content, marks, tags,
    /// task state — AND at its original sibling position (between its former
    /// neighbours, not appended at the end).
    #[tokio::test]
    async fn leaf_delete_then_undo_restores_block_and_position() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        let c1 = seed_child(&backend, &anchor, "c1", "first").await;
        let c2 = seed_child(&backend, &anchor, "c2", "middle").await;
        let c3 = seed_child(&backend, &anchor, "c3", "last").await;

        // Enrich c2: a task state (property), a tag (edge), and rich content.
        ops.set_field(&c2, "task_state", Value::String("TODO".into()))
            .await
            .expect("task_state");
        ops.set_field(
            &c2,
            "tags",
            Value::Array(vec![Value::String("Page".into())]),
        )
        .await
        .expect("tags");
        ops.set_field(&c2, "content", object_content("bold mid", &[bold(0, 4)]))
            .await
            .expect("rich content");
        ops.save_doc("").await.expect("save");

        // Order before delete: c1, c2, c3.
        assert_eq!(
            backend.list_children(&anchor).await.expect("children"),
            vec![c1.clone(), c2.clone(), c3.clone()]
        );

        let result = ops.delete(&c2).await.expect("delete c2");

        // Forward fingerprint: the `id` field present pre-delete, absent after.
        assert_eq!(result.changes.len(), 1);
        assert_eq!(result.changes[0].field, "id");

        // Inverse shape: a `create` of the SAME id, rich Object content, placed
        // after its predecessor c1.
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("leaf delete must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "create");
        assert_eq!(
            inverse.params.get("id").and_then(|v| v.as_string()),
            Some(c2.as_str())
        );
        assert!(
            matches!(inverse.params.get("content"), Some(Value::Object(_))),
            "rich block ⇒ Object content payload in the inverse"
        );
        assert_eq!(
            inverse.params.get("after").and_then(|v| v.as_string()),
            Some(c1.as_str()),
            "position anchor = predecessor sibling"
        );

        // c2 is gone.
        assert_eq!(
            backend.list_children(&anchor).await.expect("children"),
            vec![c1.clone(), c3.clone()]
        );

        replay_inverse(&ops, &inverse).await;

        // Byte-identical restore.
        let restored = backend.get_block(&c2).await.expect("restored");
        assert_eq!(restored.content, "bold mid");
        assert_eq!(restored.marks, Some(vec![bold(0, 4)]));
        assert!(restored.tags.contains("Page"), "tag restored");
        assert_eq!(
            restored.get_property_str("task_state").as_deref(),
            Some("TODO"),
            "task state restored"
        );
        // And back in its original middle slot.
        assert_eq!(
            backend.list_children(&anchor).await.expect("children"),
            vec![c1, c2, c3],
            "undo restores original sibling position"
        );
    }

    /// Deleting the FIRST child and undoing must restore it at the front
    /// (`after = Null` ⇒ first-child placement), not appended at the end.
    #[tokio::test]
    async fn first_child_delete_undo_restores_at_front() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        let c1 = seed_child(&backend, &anchor, "c1", "first").await;
        let c2 = seed_child(&backend, &anchor, "c2", "second").await;
        ops.save_doc("").await.expect("save");

        let result = ops.delete(&c1).await.expect("delete c1");
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("leaf delete must be reversible, got {other:?}"),
        };
        assert_eq!(
            inverse.params.get("after"),
            Some(&Value::Null),
            "first child ⇒ Null position anchor"
        );

        replay_inverse(&ops, &inverse).await;
        assert_eq!(
            backend.list_children(&anchor).await.expect("children"),
            vec![c1, c2],
            "first child restored at the front"
        );
    }

    /// Multibyte content (mark ranges are Unicode-scalar offsets) must survive
    /// a leaf delete → undo byte-for-byte, marks included.
    #[tokio::test]
    async fn leaf_delete_undo_multibyte_roundtrip() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        let c = seed_child(&backend, &anchor, "c", "seed").await;
        let text = "äöü😀 tail";
        ops.set_field(&c, "content", object_content(text, &[bold(0, 4)]))
            .await
            .expect("rich multibyte");
        ops.save_doc("").await.expect("save");

        let result = ops.delete(&c).await.expect("delete");
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("must be reversible, got {other:?}"),
        };
        replay_inverse(&ops, &inverse).await;

        let restored = backend.get_block(&c).await.expect("restored");
        assert_eq!(restored.content, text);
        assert_eq!(restored.marks, Some(vec![bold(0, 4)]));
    }

    /// A SUBTREE delete (target has children) stays `DeclaredIrreversible` —
    /// fail-loud, never a lossy or wrong-shaped inverse.
    #[tokio::test]
    async fn bare_delete_of_non_leaf_is_refused_fail_closed() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        let parent = seed_child(&backend, &anchor, "p", "parent").await;
        let _gc = seed_child(&backend, &parent, "gc", "grandchild").await;
        ops.save_doc("").await.expect("save");

        // Destructive-delete ruling 2026-07-21: a bare `delete` NEVER cascades a
        // subtree — it fails loud and names the two explicit opt-in ops. The
        // grandchild must still be present afterwards (no partial mutation).
        let err = ops
            .delete(&parent)
            .await
            .expect_err("bare delete of a non-leaf must be refused, not cascade");
        let msg = err.to_string();
        assert!(msg.contains("delete_subtree"), "err: {msg}");
        assert!(msg.contains("delete_keep_children"), "err: {msg}");
        assert!(
            backend.get_block(&parent).await.is_ok(),
            "refused delete must leave the parent intact"
        );
    }

    /// A NAMED source block (`#+NAME:` → `source_name`) must survive a leaf
    /// `delete` → undo byte-identically: both its `source_name` AND
    /// `source_language` come back. Regression guard — the `create` inverse
    /// captured `source_name` but the new-block path never re-applied it, so a
    /// named source block resurrected nameless (name silently dropped).
    #[tokio::test]
    async fn named_source_block_delete_undo_restores_name_and_language() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        // Create a named source block as a leaf child of the anchor, via the
        // real `create` op path (content_type=source + source_language +
        // source_name), so this exercises exactly the resurrection code path.
        let mut create_params: StorageEntity = HashMap::new();
        create_params.insert("id".into(), Value::String("block:src".into()));
        create_params.insert("parent_id".into(), Value::String(anchor.clone()));
        create_params.insert("content".into(), Value::String("SELECT 1".into()));
        create_params.insert("content_type".into(), Value::String("source".into()));
        create_params.insert("source_language".into(), Value::String("holon_sql".into()));
        create_params.insert("source_name".into(), Value::String("my_named_query".into()));
        ops.execute_operation(&EntityName::new("block"), "create", create_params)
            .await
            .expect("create named source block");
        ops.save_doc("").await.expect("save");

        let src = "block:src".to_string();

        // Precondition: the block carries both name and language.
        let before = backend.get_block(&src).await.expect("read before");
        assert_eq!(before.source_name.as_deref(), Some("my_named_query"));
        assert_eq!(
            before.source_language.as_ref().map(|l| l.to_string()),
            Some("holon_sql".to_string())
        );

        let result = ops.delete(&src).await.expect("delete named source");
        let inverse = match &result.undo {
            UndoAction::Undo(op) => op.clone(),
            other => panic!("leaf source delete must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "create");
        // The captured inverse must carry source_name (the capture side).
        assert_eq!(
            inverse
                .params
                .get("source_name")
                .and_then(|v| v.as_string()),
            Some("my_named_query")
        );

        replay_inverse(&ops, &inverse).await;

        // Byte-identical restore: name AND language back.
        let restored = backend.get_block(&src).await.expect("restored");
        assert_eq!(
            restored.source_name.as_deref(),
            Some("my_named_query"),
            "source_name must be restored on undo (was silently dropped)"
        );
        assert_eq!(
            restored.source_language.as_ref().map(|l| l.to_string()),
            Some("holon_sql".to_string()),
            "source_language must be restored on undo"
        );
    }

    /// A `create` op carrying the canonical positional key `after_block_id`
    /// must place the new block immediately AFTER the named predecessor
    /// sibling, and the key must NEVER land in the `properties` blob — it is
    /// operation-control metadata, stripped at the create boundary exactly like
    /// the `SqlOperationProvider` strips `POSITION_AFTER_BLOCK_ID_PARAM`.
    #[tokio::test]
    async fn positioned_create_honors_after_block_id_and_never_leaks_it() {
        let (ops, _dir, anchor) = ops_with_anchor().await;
        let backend = ops.get_backend("").await.expect("backend");

        let c1 = seed_child(&backend, &anchor, "c1", "first").await;
        let c2 = seed_child(&backend, &anchor, "c2", "last").await;
        ops.save_doc("").await.expect("save");

        // Create `cmid` under the anchor, positioned AFTER c1 via the
        // canonical `after_block_id` positional key.
        let mut create_params: StorageEntity = HashMap::new();
        create_params.insert("id".into(), Value::String("block:cmid".into()));
        create_params.insert("parent_id".into(), Value::String(anchor.clone()));
        create_params.insert("content".into(), Value::String("middle".into()));
        create_params.insert(
            holon_api::POSITION_AFTER_BLOCK_ID_PARAM.into(),
            Value::String(c1.clone()),
        );
        ops.execute_operation(&EntityName::new("block"), "create", create_params)
            .await
            .expect("positioned create");
        ops.save_doc("").await.expect("save");

        let cmid = "block:cmid".to_string();

        // (1) Positioned exactly between c1 and c2 — NOT appended at the end.
        assert_eq!(
            backend.list_children(&anchor).await.expect("children"),
            vec![c1, cmid.clone(), c2],
            "after_block_id must place the new block immediately after its predecessor"
        );

        // (2) The positional key is operation-control metadata — stripped at
        // the create boundary, never persisted into the `properties` blob.
        let created = backend.get_block(&cmid).await.expect("read created");
        assert_eq!(
            created.get_property(holon_api::POSITION_AFTER_BLOCK_ID_PARAM),
            None,
            "after_block_id must NOT leak into the properties blob"
        );
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
        let d = holon_core::block_op_catalog::dismiss_advice_descriptor(
            &EntityName::from("block"),
            "block",
        );
        assert_eq!(d.name, "dismiss_advice");
        assert_eq!(d.entity_name, EntityName::new("block"));
        let names: Vec<&str> = d.required_params.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(names, vec!["anchor_id", "lesson_id"]);
    }
}

#[cfg(test)]
mod intent_boundary_tests {
    use std::collections::HashMap;

    use super::*;

    fn ops_over_temp_store() -> (LoroBlockOperations, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(
            dir.path().to_path_buf(),
        )));
        (LoroBlockOperations::new(store), dir)
    }

    /// C2 provenance (`inv-history-records-all-creates`): a genuine Loro-backed
    /// create MUST report one `id` field delta (Null → minted id), the same
    /// shape the SQL create path emits. The engine's `record_history`
    /// chokepoint records `block_history` op_groups from this delta stream — an
    /// empty `changes` vector made every Loro-consolidator create silently
    /// record NO history, missing the op-group floor. An upsert over an
    /// existing block stays irreversible with no delta (no phantom create).
    #[tokio::test]
    async fn create_reports_id_field_delta_for_history() {
        let (ops, _dir) = ops_over_temp_store();
        let backend = ops.get_backend("").await.expect("backend");
        let root = backend.create_placeholder_root("root").await.expect("root");
        let root_uri = EntityUri::parse_owned(root).expect("root uri");
        ops.save_doc("").await.expect("save");

        let mut fields: StorageEntity = HashMap::new();
        fields.insert("parent_id".into(), Value::String(root_uri.to_string()));
        fields.insert("content".into(), Value::String("hello".into()));
        let (block_id, result) = ops.create(fields).await.expect("create");

        assert_eq!(
            result.changes.len(),
            1,
            "a genuine create must report exactly one field delta for history, got {:?}",
            result.changes
        );
        let delta = &result.changes[0];
        assert_eq!(
            delta.entity_id, block_id,
            "delta must key the created block"
        );
        assert_eq!(delta.field, "id", "the create delta is the `id` field");
        assert_eq!(delta.old_value, Value::Null, "pre-create `id` is Null");
        assert_eq!(
            delta.new_value,
            Value::String(block_id.clone()),
            "post-create `id` is the minted id"
        );
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

#[cfg(test)]
mod tag_op_tests {
    use std::collections::HashMap;

    use holon_api::PAGE_TAG;
    use holon_api::block::BlockContent;
    use holon_api::repository::CoreOperations;
    use holon_core::traits::UndoAction;

    use super::*;

    /// Ops over a temp store with a root, returning ops + the backend.
    async fn ops_and_backend() -> (LoroBlockOperations, tempfile::TempDir, LoroBackend) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(
            dir.path().to_path_buf(),
        )));
        let ops = LoroBlockOperations::new(store);
        let backend = ops.get_backend("").await.expect("backend");
        backend.create_placeholder_root("root").await.expect("root");
        (ops, dir, backend)
    }

    /// Create a block with an explicit id under `parent`.
    async fn make_block(backend: &LoroBackend, id: &str, parent: EntityUri) {
        backend
            .create_block(parent, BlockContent::text(id), Some(EntityUri::block(id)))
            .await
            .unwrap_or_else(|e| panic!("create {id}: {e}"));
    }

    async fn run(ops: &LoroBlockOperations, op: &str, id: &str, tag: &str) -> OperationResult {
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String(id.to_string()));
        params.insert("tag".into(), Value::String(tag.to_string()));
        ops.execute_operation(&EntityName::new("block"), op, params)
            .await
            .unwrap_or_else(|e| panic!("{op} failed: {e}"))
    }

    async fn tags_of(backend: &LoroBackend, id: &str) -> Vec<String> {
        backend
            .get_block(id)
            .await
            .expect("get_block")
            .tags
            .to_vec()
    }

    fn is_vacuous(r: &OperationResult) -> bool {
        !r.changes.is_empty() && r.changes.iter().all(|d| d.old_value == d.new_value)
    }

    #[tokio::test]
    async fn add_tag_is_idempotent() {
        let (ops, _dir, backend) = ops_and_backend().await;
        make_block(&backend, "x", EntityUri::no_parent()).await;
        ops.save_doc("").await.expect("save");

        let first = run(&ops, "add_tag", "block:x", "todo").await;
        assert_eq!(tags_of(&backend, "block:x").await, vec!["todo".to_string()]);
        assert!(!is_vacuous(&first));

        let second = run(&ops, "add_tag", "block:x", "todo").await;
        assert_eq!(tags_of(&backend, "block:x").await, vec!["todo".to_string()]);
        assert!(is_vacuous(&second), "re-add of a present tag is vacuous");
    }

    #[tokio::test]
    async fn remove_tag_is_targeted_and_idempotent() {
        let (ops, _dir, backend) = ops_and_backend().await;
        make_block(&backend, "x", EntityUri::no_parent()).await;
        ops.save_doc("").await.expect("save");
        run(&ops, "add_tag", "block:x", "a").await;
        run(&ops, "add_tag", "block:x", "b").await;

        run(&ops, "remove_tag", "block:x", "a").await;
        assert_eq!(tags_of(&backend, "block:x").await, vec!["b".to_string()]);

        let noop = run(&ops, "remove_tag", "block:x", "a").await;
        assert!(is_vacuous(&noop));
        assert_eq!(tags_of(&backend, "block:x").await, vec!["b".to_string()]);
    }

    #[tokio::test]
    async fn add_tag_inverse_round_trips() {
        let (ops, _dir, backend) = ops_and_backend().await;
        make_block(&backend, "x", EntityUri::no_parent()).await;
        ops.save_doc("").await.expect("save");

        let result = run(&ops, "add_tag", "block:x", "todo").await;
        let inverse = match result.undo {
            UndoAction::Undo(op) => op,
            other => panic!("add_tag must be reversible, got {other:?}"),
        };
        assert_eq!(inverse.op_name, "remove_tag");
        let mut params: StorageEntity = HashMap::new();
        for (k, v) in &inverse.params {
            params.insert(k.as_str().into(), v.clone());
        }
        ops.execute_operation(&EntityName::new("block"), &inverse.op_name, params)
            .await
            .expect("replay inverse");
        assert!(tags_of(&backend, "block:x").await.is_empty());
    }

    /// Page guard: marking a block Page under a non-page parent is rejected;
    /// a block at no_parent (seed page) is allowed; a page under a page
    /// allowed.
    #[tokio::test]
    async fn add_page_tag_nesting_guard() {
        let (ops, _dir, backend) = ops_and_backend().await;
        make_block(&backend, "parent", EntityUri::no_parent()).await;
        make_block(&backend, "child", EntityUri::block("parent")).await;
        ops.save_doc("").await.expect("save");

        // Parent is a non-page block → reject.
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String("block:child".to_string()));
        params.insert("tag".into(), Value::String(PAGE_TAG.to_string()));
        let err = ops
            .execute_operation(&EntityName::new("block"), "add_tag", params)
            .await
            .expect_err("page under non-page must be rejected");
        assert!(
            err.to_string().contains("pages under non-pages"),
            "got: {err}"
        );

        // Seed page at no_parent → allowed; then child under the now-page parent.
        run(&ops, "add_tag", "block:parent", PAGE_TAG).await;
        assert!(
            tags_of(&backend, "block:parent")
                .await
                .contains(&PAGE_TAG.to_string())
        );
        run(&ops, "add_tag", "block:child", PAGE_TAG).await;
        assert!(
            tags_of(&backend, "block:child")
                .await
                .contains(&PAGE_TAG.to_string())
        );
    }

    #[tokio::test]
    async fn remove_page_tag_with_page_child_rejected() {
        let (ops, _dir, backend) = ops_and_backend().await;
        make_block(&backend, "parent", EntityUri::no_parent()).await;
        make_block(&backend, "child", EntityUri::block("parent")).await;
        ops.save_doc("").await.expect("save");
        run(&ops, "add_tag", "block:parent", PAGE_TAG).await;
        run(&ops, "add_tag", "block:child", PAGE_TAG).await;

        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String("block:parent".to_string()));
        params.insert("tag".into(), Value::String(PAGE_TAG.to_string()));
        let err = ops
            .execute_operation(&EntityName::new("block"), "remove_tag", params)
            .await
            .expect_err("removing Page with a page child must be rejected");
        assert!(
            err.to_string().contains("page under a non-page"),
            "got: {err}"
        );
    }

    #[tokio::test]
    async fn add_tag_on_missing_block_fails_loud() {
        let (ops, _dir, _backend) = ops_and_backend().await;
        let mut params: StorageEntity = HashMap::new();
        params.insert("id".into(), Value::String("block:ghost".to_string()));
        params.insert("tag".into(), Value::String("todo".to_string()));
        let err = ops
            .execute_operation(&EntityName::new("block"), "add_tag", params)
            .await
            .expect_err("add_tag on a missing block must fail loud");
        assert!(err.to_string().contains("not found"), "got: {err}");
    }
}

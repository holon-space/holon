//! **Loro** seam adapters for the backend-blind file-sync controller.
//!
//! ADR 0004 Phase 9: the file-sync controller speaks only to the
//! [`BlockReader`], [`DocumentManager`], and [`BlockOrdering`] seams. These
//! three adapters implement those seams directly against a [`LoroBackend`] tree
//! — no Turso, no `QueryableCache`, no cell registry, no command bus. They are
//! the Loro counterparts of `CacheBlockReader` / `LiveDocumentManager` (Turso)
//! in [`crate::turso_seams`] and of the `Upstream` branch of `impl
//! BlockOrdering for SqlBlockOperations`.
//!
//! These are registered as the `dyn BlockReader` / `dyn DocumentManager` /
//! `dyn BlockOrdering` providers in the no-Turso container; resolving
//! [`holon_orgmode::di::FileSyncStarted`] then self-starts the controller over
//! them — the same DI-driven path the Turso container uses, no `spawn_*` call.
//!
//! They live in holon-app (Phase 2.5, C1/C2 of the architecture plan): only the
//! wiring crate may name a concrete backend type, so holon-orgmode stays
//! storage-agnostic and the adapters are constructed here at wiring time.
//!
//! Sibling order is the Loro tree's own child order (the fractional index the
//! SQL projection would otherwise materialize as `sort_key`); `list_children`
//! returns children in that canonical order and `get_blocks` preserves it.

use std::collections::HashMap;
use std::future::Future;
use std::path::Path;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use async_trait::async_trait;
use holon::api::repository::CoreOperations;
use holon::api::types::Traversal;
use holon::sync::LoroDocumentStore;
use holon_api::BlockContent;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::capability::Consolidator;
use holon_api::entity_uri::EntityUri;
use holon_api::types::ContentType;
use holon_api::types::Tags;
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as BlockOrderingResult;
use holon_filesystem::AliasRegistrar;
use holon_filesystem::BlockReader;
use holon_filesystem::DocumentManager;
use holon_loro::LoroBackend;
use tokio::sync::RwLock;

/// Box a Loro `ApiError` (or any error) into the `BlockOrdering` error type,
/// preserving the full chain via `{e:#}` — mirrors `SqlBlockOperations`.
fn boxed<E: std::fmt::Display>(e: E) -> Box<dyn std::error::Error + Send + Sync> {
    format!("{e:#}").into()
}

/// Build the `BlockContent` for a domain `Block` exactly as
/// `LoroBlockOperations::apply_create` does: Source content carries the
/// language, everything else is plain text.
fn block_content_for(doc: &Block) -> BlockContent {
    if doc.content_type == ContentType::Source {
        let lang = doc
            .source_language
            .as_ref()
            .map(|l| l.to_string())
            .unwrap_or_else(|| "text".to_string());
        BlockContent::source(&lang, doc.content.clone())
    } else {
        BlockContent::text(doc.content.clone())
    }
}

/// Parse the `tags`/`requires` edge field from a `build_block_params` map
/// value, which the org reconciler emits either as a `Value::Array` or a CSV
/// string.
fn parse_string_list(value: &Value) -> BlockOrderingResult<Vec<String>> {
    match value {
        Value::Array(items) => Ok(items
            .iter()
            .filter_map(|v| v.as_string().map(str::to_string))
            .collect()),
        Value::String(s) if s.is_empty() => Ok(Vec::new()),
        Value::String(s) => Ok(s.split(',').map(|t| t.trim().to_string()).collect()),
        other => Err(boxed(format!("unsupported string-list shape {other:?}"))),
    }
}

// ============================================================================
// LoroBlockReader
// ============================================================================

pub struct LoroBlockReader {
    backend: Arc<LoroBackend>,
}

impl LoroBlockReader {
    pub fn new(backend: Arc<LoroBackend>) -> Self {
        Self { backend }
    }

    /// Append `parent`'s subtree to `out` in pre-order, siblings in Loro tree
    /// order, **excluding Page-tagged blocks** (and not descending into them):
    /// a Page is a sub-document boundary, mirroring the Turso recursive CTE
    /// that `LEFT JOIN block_tags ... tag='Page' WHERE bt.block_id IS
    /// NULL`.
    fn collect_subtree<'a>(
        &'a self,
        parent: &'a EntityUri,
        out: &'a mut Vec<Block>,
    ) -> Pin<Box<dyn Future<Output = AnyhowResult<()>> + Send + 'a>> {
        Box::pin(async move {
            let child_ids = self.backend.list_children(parent.as_str()).await?;
            let children = self.backend.get_blocks(child_ids).await?;
            for child in children {
                if child.is_page() {
                    continue;
                }
                let child_id = child.id.clone();
                out.push(child);
                self.collect_subtree(&child_id, out).await?;
            }
            Ok(())
        })
    }
}

#[async_trait]
impl BlockReader for LoroBlockReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> AnyhowResult<Vec<Block>> {
        let mut out = Vec::new();
        self.collect_subtree(doc_id, &mut out).await?;
        Ok(out)
    }

    async fn doc_block_topology(
        &self,
        doc_id: &EntityUri,
    ) -> AnyhowResult<Vec<(EntityUri, EntityUri)>> {
        // Honest, not a stub: the same subtree walk `get_blocks` does, reporting
        // each node's real tree parent. The Loro tree has no projection to
        // exploit — nodes carry their content inline — so this costs what
        // `get_blocks` costs and simply drops the bodies. The saving that
        // motivates this method is Turso-side.
        let mut out = Vec::new();
        self.collect_subtree(doc_id, &mut out).await?;
        Ok(out.into_iter().map(|b| (b.id, b.parent_id)).collect())
    }

    async fn get_block_authoritative(&self, id: &EntityUri) -> AnyhowResult<Option<Block>> {
        // The Loro tree is the write authority (no matview/`block_raw` split);
        // a direct node read is the authoritative O(1) point lookup used by the
        // org-writeback incremental cache's per-edit refresh.
        match self.backend.get_block(id.as_str()).await {
            Ok(block) => Ok(Some(block)),
            Err(holon_api::ApiError::BlockNotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    async fn resolve_link_marks(&self, _: &mut [Block]) -> AnyhowResult<()> {
        // Nothing to resolve against: link resolution is recorded in the
        // `block_links` junction, which is a Turso table. The Loro tree stores
        // marks exactly as authored and holds no name -> id index.
        //
        // Consequence, stated rather than hidden: in a Loro-backed (no-Turso)
        // wiring a dangling `[[Some Page]]` renders as authored and never
        // upgrades to `[[block:<id>][Some Page]]`. That is the truthful output
        // for a store that does not know the link resolved — not a dropped
        // upgrade. `LoroBlockReader` is wired only by the no-Turso test
        // container, where the junction does not exist at all.
        Ok(())
    }

    async fn iter_documents_with_blocks(&self) -> AnyhowResult<Vec<(EntityUri, Vec<Block>)>> {
        // The implicit root document: top-level blocks live directly under the
        // tree root (`no_parent`), excluding any Page roots which are returned
        // as their own documents below.
        let mut docs = vec![(
            EntityUri::no_parent(),
            self.get_blocks(&EntityUri::no_parent()).await?,
        )];

        let all = self.backend.get_all_blocks(Traversal::ALL_BUT_ROOT).await?;
        for block in all {
            if block.is_page() {
                let page_id = block.id.clone();
                let blocks = self.get_blocks(&page_id).await?;
                docs.push((page_id, blocks));
            }
        }
        Ok(docs)
    }

    /// No SQL `file` table under Loro — nothing to load.
    async fn load_file_hashes(&self) -> AnyhowResult<Vec<(EntityUri, String)>> {
        Ok(vec![])
    }

    /// No SQL `file` table under Loro — nothing to persist.
    async fn persist_file_hash(&self, _: &EntityUri, _: &str) -> AnyhowResult<()> {
        Ok(())
    }

    /// No `LiveData<Block>` CDC feed under Loro: writes are synchronous against
    /// the tree, so every id should already be resolvable. Verify presence via
    /// `get_block` rather than blindly returning `true`.
    async fn wait_for_blocks_in_feed(&self, block_ids: &[String], _: u64) -> bool {
        for id in block_ids {
            if self.backend.get_block(id).await.is_err() {
                return false;
            }
        }
        true
    }

    /// Synchronous tree — presence-count via the same `get_block` probe.
    async fn blocks_in_feed_count(&self, block_ids: &[String]) -> usize {
        let mut present = 0;
        for id in block_ids {
            if self.backend.get_block(id).await.is_ok() {
                present += 1;
            }
        }
        present
    }

    // find_foreign_blocks: trait default over iter_documents_with_blocks is
    // correct.
}

// ============================================================================
// LoroDocumentManager
// ============================================================================

pub struct LoroDocumentManager {
    backend: Arc<LoroBackend>,
}

impl LoroDocumentManager {
    pub fn new(backend: Arc<LoroBackend>) -> Self {
        Self { backend }
    }
}

#[async_trait]
impl DocumentManager for LoroDocumentManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> AnyhowResult<Option<Block>> {
        let child_ids = self.backend.list_children(parent_id.as_str()).await?;
        let children = self.backend.get_blocks(child_ids).await?;
        Ok(children
            .into_iter()
            .find(|b| b.is_page() && b.title() == title))
    }

    async fn get_by_id(&self, id: &EntityUri) -> AnyhowResult<Option<Block>> {
        match self.backend.get_block(id.as_str()).await {
            Ok(block) => Ok(Some(block)),
            Err(holon_api::ApiError::BlockNotFound { .. }) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    /// Create a page block directly in the Loro tree.
    ///
    /// GOTCHA: tags MUST be set via `set_block_tags` (the tree-meta edge), not
    /// routed through the operation provider — that path mis-stores the `Page`
    /// tag as a plain property and the block never registers as a document.
    async fn create(&self, doc: Block) -> AnyhowResult<Block> {
        let content = block_content_for(&doc);
        let created = self
            .backend
            .create_block(doc.parent_id.clone(), content, Some(doc.id.clone()))
            .await?;

        self.backend
            .set_block_tags(created.id.as_str(), &doc.tags.to_vec())
            .await?;

        if !doc.properties.is_empty() {
            self.backend
                .update_block_properties(created.id.as_str(), &doc.properties)
                .await?;
        }

        Ok(self.backend.get_block(created.id.as_str()).await?)
    }

    /// Persist doc-level metadata (e.g. `todo_keywords`) onto the page block.
    ///
    /// `doc.properties` is the AUTHORITATIVE full set (callers fetch the doc,
    /// mutate, and pass it back), so this replaces rather than merges — a merge
    /// could never clear a removed key (e.g. `todo_keywords` after the
    /// `#+TODO:` header was deleted from the org file), and an empty set is a
    /// meaningful "remove everything", not a skip.
    async fn update_metadata(&self, doc: &Block) -> AnyhowResult<()> {
        self.backend
            .replace_block_properties(doc.id.as_str(), &doc.properties)
            .await?;
        self.backend
            .set_block_tags(doc.id.as_str(), &doc.tags.to_vec())
            .await?;
        // The doc-root's own content carries its pre-first-headline body, which
        // write-back renders above the first headline. Persisting only
        // properties+tags here dropped it, so the Loro wiring deleted the root
        // body from disk on every write-back while the Turso wiring kept it.
        self.backend
            .update_block_text(doc.id.as_str(), &doc.content)
            .await?;
        Ok(())
    }

    // name_chain / find_by_name_chain / get_or_create_by_name_chain: trait
    // defaults build on the required methods above.
}

// ============================================================================
// LoroBlockOrdering
// ============================================================================

pub struct LoroBlockOrdering {
    backend: Arc<LoroBackend>,
}

impl LoroBlockOrdering {
    pub fn new(backend: Arc<LoroBackend>) -> Self {
        Self { backend }
    }

    async fn children_uris(&self, parent_id: &EntityUri) -> BlockOrderingResult<Vec<EntityUri>> {
        let ids = self
            .backend
            .list_children(parent_id.as_str())
            .await
            .map_err(boxed)?;
        // ALLOW(entity_uri_from_raw): ids from backend.list_children() (Vec<String>
        // Loro output)
        Ok(ids.iter().map(|s| EntityUri::from_raw(s)).collect())
    }

    /// The block's parent uri, or `None` when it has no block parent (root).
    async fn parent_of(&self, id: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        let block = self.backend.get_block(id.as_str()).await.map_err(boxed)?;
        let parent = block.parent_id;
        if parent == EntityUri::no_parent() {
            Ok(None)
        } else {
            Ok(Some(parent))
        }
    }
}

#[async_trait]
impl BlockOrdering for LoroBlockOrdering {
    async fn place(
        &self,
        uri: &EntityUri,
        parent_id: &EntityUri,
        after_id: Option<&EntityUri>,
    ) -> BlockOrderingResult<()> {
        self.backend
            .update_block_position(
                uri.as_str(),
                parent_id.as_str(),
                after_id.map(|u| u.as_str()),
            )
            .await
            .map_err(boxed)
    }

    async fn place_all(
        &self,
        parent_id: &EntityUri,
        ordered_ids: &[EntityUri],
    ) -> BlockOrderingResult<()> {
        let mut prev: Option<&EntityUri> = None;
        for id in ordered_ids {
            self.place(id, parent_id, prev).await?;
            prev = Some(id);
        }
        Ok(())
    }

    async fn prev_sibling(&self, id: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        let parent = match self.parent_of(id).await? {
            Some(p) => p,
            None => return Ok(None),
        };
        let siblings = self.children_uris(&parent).await?;
        let pos = siblings.iter().position(|s| s == id);
        Ok(match pos {
            Some(i) if i > 0 => Some(siblings[i - 1].clone()),
            _ => None,
        })
    }

    async fn next_sibling(&self, id: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        let parent = match self.parent_of(id).await? {
            Some(p) => p,
            None => return Ok(None),
        };
        let siblings = self.children_uris(&parent).await?;
        let pos = siblings.iter().position(|s| s == id);
        Ok(match pos {
            Some(i) => siblings.get(i + 1).cloned(),
            None => None,
        })
    }

    async fn first_child(&self, parent_id: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        Ok(self.children_uris(parent_id).await?.into_iter().next())
    }

    async fn last_child(&self, parent_id: &EntityUri) -> BlockOrderingResult<Option<EntityUri>> {
        Ok(self.children_uris(parent_id).await?.into_iter().last())
    }

    async fn children(&self, parent_id: &EntityUri) -> BlockOrderingResult<Vec<EntityUri>> {
        self.children_uris(parent_id).await
    }

    /// No-Turso runs in the **degraded / Store-consolidator** model: the Loro
    /// tree is both the order authority and the only sink, so there is no
    /// separate downstream projection to flush a consolidator-persisted create
    /// to. Returning `false` tells the controller "the store is itself the
    /// consolidator" — it then routes every create through [`update_in_tree`]
    /// (which writes directly to Loro), exactly as the SqlOnly arm does. (For
    /// the document block this is also the intended path: the doc manager owns
    /// that row — see `file_sync_controller.rs:406`.)
    async fn create_in_tree(
        &self,
        _: &EntityUri,
        _: Option<&EntityUri>,
        _: &EntityUri,
        _: BlockContent,
        _: &HashMap<String, Value>,
        _: &Tags,
        _: &[EntityUri],
        _: &[EntityUri],
    ) -> BlockOrderingResult<bool> {
        Ok(false)
    }

    /// The single org→block write seam in the degraded/Store model. Both the
    /// controller's `"create"` and `"update"` ops land here (see
    /// `file_sync_controller.rs:744`), so this **upserts**: a block the Loro
    /// tree doesn't have yet is created (with its typed content under
    /// `parent_id`), an existing one has its content replaced; either way
    /// tags/requires/ properties are written and the block is positioned
    /// via `place`. Writes go straight to `LoroBackend` (no cell registry,
    /// no SQL, no command bus).
    async fn update_in_tree(
        &self,
        mut params: holon_api::StorageEntity,
    ) -> BlockOrderingResult<()> {
        let id = params
            .remove("id")
            .and_then(|v| v.as_string().map(str::to_string))
            .ok_or_else(|| boxed("update_in_tree: missing 'id' param"))?;
        let after = params
            .remove(holon::sync::event_bus::POSITION_AFTER_BLOCK_ID_PARAM)
            .and_then(|v| v.as_string().map(str::to_string));
        params.remove(holon::sync::event_bus::ROUTING_DOC_URI_KEY);
        let parent_id = params
            .remove("parent_id")
            .and_then(|v| v.as_string().map(str::to_string));

        // Content is interdependent with content_type/source_language, so pull
        // all three out and build one typed `BlockContent` rather than treating
        // `content` as a bare text field (which would degrade a `#+BEGIN_SRC`).
        let content = params
            .remove("content")
            .and_then(|v| v.as_string().map(str::to_string));
        let content_type = match params.remove("content_type") {
            Some(v) => {
                let s = v
                    .as_string()
                    .ok_or_else(|| boxed("update_in_tree: 'content_type' must be a string"))?;
                s.parse::<ContentType>()
                    .map_err(|_| boxed(format!("update_in_tree: invalid content_type {s:?}")))?
            }
            None => ContentType::Text,
        };
        let source_language = params
            .remove("source_language")
            .and_then(|v| v.as_string().map(str::to_string));
        let tags = match params.remove("tags") {
            Some(v) => Some(parse_string_list(&v).map_err(|e| boxed(format!("'tags': {e}")))?),
            None => None,
        };
        let requires = match params.remove("requires") {
            Some(v) => Some(parse_string_list(&v).map_err(|e| boxed(format!("'requires': {e}")))?),
            None => None,
        };
        let advice_suppressed = match params.remove("advice_suppressed") {
            Some(v) => Some(
                parse_string_list(&v).map_err(|e| boxed(format!("'advice_suppressed': {e}")))?,
            ),
            None => None,
        };
        // Everything remaining is a property.
        let properties = params;

        let built_content = if content_type == ContentType::Source {
            let lang = source_language.as_deref().unwrap_or("text");
            BlockContent::source(lang, content.clone().unwrap_or_default())
        } else {
            BlockContent::text(content.clone().unwrap_or_default())
        };

        // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param
        // HashMap<String,Value> bag
        let block_uri = EntityUri::from_raw(&id);
        let exists = self.backend.get_block(&id).await.is_ok();
        if !exists {
            let parent = parent_id
                .clone()
                .ok_or_else(|| boxed("update_in_tree: create requires a 'parent_id'"))?;
            self.backend
                .create_block(
                    // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param
                    // HashMap<String,Value> bag
                    EntityUri::from_raw(&parent),
                    built_content,
                    Some(block_uri.clone()),
                )
                .await
                .map_err(boxed)?;
        } else if content.is_some() {
            self.backend
                .update_block(&id, built_content)
                .await
                .map_err(boxed)?;
        }

        if let Some(tags) = tags {
            self.backend
                .set_block_tags(&id, &tags)
                .await
                .map_err(boxed)?;
        }
        if let Some(requires) = requires {
            let requires: Vec<EntityUri> = requires
                .into_iter()
                .map(|r| {
                    EntityUri::parse_owned(r)
                        .map_err(|e| boxed(format!("update_in_tree: invalid 'requires' URI: {e}")))
                })
                .collect::<Result<_, _>>()?;
            self.backend
                .set_block_requires(&id, &requires)
                .await
                .map_err(boxed)?;
        }
        if let Some(advice_suppressed) = advice_suppressed {
            let advice_suppressed: Vec<EntityUri> = advice_suppressed
                .into_iter()
                .map(|r| {
                    EntityUri::parse_owned(r).map_err(|e| {
                        boxed(format!(
                            "update_in_tree: invalid 'advice_suppressed' URI: {e}"
                        ))
                    })
                })
                .collect::<Result<_, _>>()?;
            self.backend
                .set_block_advice_suppressed(&id, &advice_suppressed)
                .await
                .map_err(boxed)?;
        }
        if !properties.is_empty() {
            // LoroBackend's property maps are String-keyed (serde surface);
            // re-key the Arc<str> param bag at this boundary.
            let properties: HashMap<String, Value> = properties
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect();
            self.backend
                .update_block_properties(&id, &properties)
                .await
                .map_err(boxed)?;
        }

        match (parent_id, after) {
            (Some(parent), Some(after_id)) => {
                // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param
                // HashMap<String,Value> bag
                let parent_uri = EntityUri::from_raw(&parent);
                // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param
                // HashMap<String,Value> bag
                let after_uri = EntityUri::from_raw(&after_id);
                self.place(&block_uri, &parent_uri, Some(&after_uri))
                    .await?;
            }
            // No recorded predecessor: place as first child under the parent;
            // the org reconciler's disk-order replay finalises sibling order.
            (Some(parent), None) => {
                // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param
                // HashMap<String,Value> bag
                let parent_uri = EntityUri::from_raw(&parent);
                self.place(&block_uri, &parent_uri, None).await?;
            }
            (None, _) => {}
        }
        Ok(())
    }

    async fn delete_in_tree(&self, params: holon_api::StorageEntity) -> BlockOrderingResult<()> {
        let id = params
            .get("id")
            .and_then(|v| v.as_string())
            .ok_or_else(|| boxed("delete_in_tree: missing 'id' param"))?;
        self.backend.delete_block(id).await.map_err(boxed)
    }

    /// No separate upstream consolidator: the Loro tree is the store AND the
    /// order authority, written directly by `update_in_tree`. There is no
    /// downstream sink projection to flush to (cf.
    /// `file_sync_controller.rs:988`).
    fn has_upstream_consolidator(&self) -> bool {
        false
    }

    fn consolidator(&self) -> Consolidator {
        Consolidator::Store
    }
}

/// `AliasRegistrar` backed by `LoroDocumentStore` — registers and resolves the
/// `doc_id ↔ path` aliases the file-sync controller needs. Must share the same
/// `Arc<RwLock<LoroDocumentStore>>` as the other Loro seams /
/// `LoroBlockOperations`.
pub struct LoroAliasRegistrar {
    pub doc_store: Arc<RwLock<LoroDocumentStore>>,
}

#[async_trait]
impl AliasRegistrar for LoroAliasRegistrar {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path) {
        let store = self.doc_store.read().await;
        store.register_alias(doc_id.as_str(), path).await;
    }

    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        let store = self.doc_store.read().await;
        store.resolve_alias_to_path(doc_id.as_str()).await
    }
}

/// `ShareWritebackDisclosure` (Inc 1) that forwards a shared-subtree
/// write-back gap to the `DegradedSignalBus`, so the frontend renders a
/// degraded banner instead of the edit silently failing to reach disk. Lives
/// in the wiring crate because it bridges the storage-agnostic port
/// (holon-filesystem) to the concrete bus (holon-loro/holon).
pub struct ShareDegradedDisclosure {
    pub bus: Arc<holon::sync::DegradedSignalBus>,
}

impl holon_filesystem::ShareWritebackDisclosure for ShareDegradedDisclosure {
    fn shared_subtree_not_materialized(&self, block_id: &EntityUri, shared_tree_id: &str) {
        self.bus.emit(holon::sync::ShareDegraded {
            shared_tree_id: shared_tree_id.to_string(),
            reason: holon::sync::ShareDegradedReason::SharedSubtreeNotMaterialized(
                block_id.to_string(),
            ),
        });
    }
}

/// `WritebackDisclosure` that turns the write-back supervisor's give-up into
/// the `WritebackDegraded` banner. Same bridging role as
/// [`ShareDegradedDisclosure`]: the supervisor lives in holon-orgmode, which
/// has no view of the concrete bus.
pub struct WritebackDegradedDisclosure {
    pub bus: Arc<holon::sync::DegradedSignalBus>,
}

impl holon_filesystem::WritebackDisclosure for WritebackDegradedDisclosure {
    fn writeback_degraded(&self, detail: &str) {
        self.bus.emit(holon::sync::ShareDegraded {
            // Not a share condition; the sentinel subject the
            // `WritebackDegraded` variant documents keeps one process-wide
            // banner instead of one per share.
            shared_tree_id: "org-writeback".to_string(),
            reason: holon::sync::ShareDegradedReason::WritebackDegraded(detail.to_string()),
        });
    }
}

/// `MountRegistry` (Inc 3) backed by the global Loro tree's mount nodes — the
/// authoritative, non-user-authorable signal for "is this id a real shared
/// subtree mount?". Delegates to `LoroShareBackend::is_registered_mount`.
pub struct LoroMountRegistry {
    pub backend: Arc<holon_loro::loro_share_backend::LoroShareBackend>,
}

#[async_trait]
impl holon_filesystem::MountRegistry for LoroMountRegistry {
    async fn is_registered_mount(&self, block_id: &EntityUri) -> anyhow::Result<bool> {
        self.backend
            .is_registered_mount(block_id.as_str())
            .await
            .map_err(|e| anyhow::anyhow!("mount registry check for {block_id}: {e}"))
    }
}

#[cfg(test)]
mod order_minting_type_level {
    //! Type-level proof for spec 0008 §3.2 (Replication.md §5): the Loro
    //! ordering seam owns the positional [`BlockOrdering`] surface but is, by
    //! construction, UNABLE to mint order keys — it does not implement
    //! [`OrderKeyMinting`], and that trait carries the only `-> String`
    //! key-minting method. The former placeholder `new_child_anchor` that
    //! returned a `default_sort_key()` here is gone and cannot be re-added
    //! without also (illegally) implementing `OrderKeyMinting` for a Loro seam.
    use holon_core::block_ordering::BlockOrdering;
    use holon_core::block_ordering::OrderKeyMinting;

    use super::LoroBlockOrdering;

    // Positive: the Loro seam IS a positional ordering provider.
    fn _is_block_ordering(x: &LoroBlockOrdering) -> &dyn BlockOrdering {
        x
    }

    // Negative (must NOT compile): the Loro seam is not a key minter. Keeping
    // this as a compile-fail witness — uncommenting it is a hard error because
    // `LoroBlockOrdering: !OrderKeyMinting`.
    //
    // fn _is_not_a_minter(x: &LoroBlockOrdering) -> &dyn OrderKeyMinting { x }

    // Referenced so the import can't rot into a warning if the negative witness
    // above is edited; the trait is intentionally never coerced-to here.
    const _: fn() = || {
        fn _assert_object_safe(_: &dyn OrderKeyMinting) {}
    };
}

//! Loro-backed adapters for the backend-blind file-sync controller, plus the
//! no-Turso org-sync bootstrap (`spawn_loro_org_sync`).
//!
//! ADR 0004 Phase 9, task #4: the org sync controller speaks only to the
//! [`BlockReader`], [`DocumentManager`], and [`BlockOrdering`] seams. These three
//! adapters implement those seams directly against a [`LoroBackend`] tree — no
//! Turso, no `QueryableCache`, no cell registry, no command bus. They are the
//! Loro counterparts of `CacheBlockReader` / `LiveDocumentManager` (Turso) in
//! holon-orgmode's `di.rs` and of the `Upstream` branch of `impl BlockOrdering
//! for SqlBlockOperations`.
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
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use anyhow::Result as AnyhowResult;
use async_trait::async_trait;
use holon::api::loro_backend::LoroBackend;
use holon::api::repository::CoreOperations;
use holon::api::types::Traversal;
use holon_api::block::Block;
use holon_api::capability::Consolidator;
use holon_api::entity_uri::EntityUri;
use holon_api::types::{ContentType, Tags};
use holon_api::{BlockContent, Value};
use holon_core::block_ordering::BlockOrdering;
use holon_core::traits::Result as BlockOrderingResult;

use holon_orgmode::traits::{BlockReader, DocumentManager};

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

/// Parse the `tags`/`requires` edge field from a `build_block_params` map value,
/// which the org reconciler emits either as a `Value::Array` or a CSV string.
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
    /// a Page is a sub-document boundary, mirroring the Turso recursive CTE that
    /// `LEFT JOIN block_tags ... tag='Page' WHERE bt.block_id IS NULL`.
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

    // find_foreign_blocks: trait default over iter_documents_with_blocks is correct.
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
        // ALLOW(entity_uri_from_raw): ids from backend.list_children() (Vec<String> Loro output)
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

    /// Loro mints the real fractional index when the block is placed; the value
    /// returned here is overwritten, so a placeholder default key suffices.
    async fn new_child_anchor(
        &self,
        _: &EntityUri,
        _: Option<&EntityUri>,
    ) -> BlockOrderingResult<String> {
        Ok(holon_core::fractional_index::default_sort_key())
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
        _: &[String],
    ) -> BlockOrderingResult<bool> {
        Ok(false)
    }

    /// The single org→block write seam in the degraded/Store model. Both the
    /// controller's `"create"` and `"update"` ops land here (see
    /// `file_sync_controller.rs:744`), so this **upserts**: a block the Loro tree
    /// doesn't have yet is created (with its typed content under `parent_id`),
    /// an existing one has its content replaced; either way tags/requires/
    /// properties are written and the block is positioned via `place`. Writes go
    /// straight to `LoroBackend` (no cell registry, no SQL, no command bus).
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
        // Everything remaining is a property.
        let properties = params;

        let built_content = if content_type == ContentType::Source {
            let lang = source_language.as_deref().unwrap_or("text");
            BlockContent::source(lang, content.clone().unwrap_or_default())
        } else {
            BlockContent::text(content.clone().unwrap_or_default())
        };

        // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param HashMap<String,Value> bag
        let block_uri = EntityUri::from_raw(&id);
        let exists = self.backend.get_block(&id).await.is_ok();
        if !exists {
            let parent = parent_id
                .clone()
                .ok_or_else(|| boxed("update_in_tree: create requires a 'parent_id'"))?;
            self.backend
                .create_block(
                    // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param HashMap<String,Value> bag
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
            self.backend
                .set_block_requires(&id, &requires)
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
                // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param HashMap<String,Value> bag
                let parent_uri = EntityUri::from_raw(&parent);
                // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param HashMap<String,Value> bag
                let after_uri = EntityUri::from_raw(&after_id);
                self.place(&block_uri, &parent_uri, Some(&after_uri))
                    .await?;
            }
            // No recorded predecessor: place as first child under the parent;
            // the org reconciler's disk-order replay finalises sibling order.
            (Some(parent), None) => {
                // ALLOW(entity_uri_from_raw): id/parent_id/after_id from operation param HashMap<String,Value> bag
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
    /// downstream sink projection to flush to (cf. `file_sync_controller.rs:988`).
    fn has_upstream_consolidator(&self) -> bool {
        false
    }

    fn consolidator(&self) -> Consolidator {
        Consolidator::Store
    }

    /// Loro owns the order key; there is no SQL `sort_key` sink to project to.
    async fn project_sort_keys(&self, _: &[EntityUri]) -> BlockOrderingResult<()> {
        Ok(())
    }
}

/// Build the three Loro adapters, construct an [`FileSyncController`] over them,
/// and spawn [`run_file_sync_controller`] — the no-Turso bootstrap. The returned
/// strong `Arc<OrgSyncIdleSignal>` keeps the loop alive (the loop holds a `Weak`
/// and exits when the caller drops it). Called from within an async tokio
/// context (the no-Turso test setup) so `tokio::spawn` has a runtime.
///
/// [`FileSyncController`]: holon_orgmode::file_sync_controller::FileSyncController
/// [`run_file_sync_controller`]: holon_orgmode::di::run_file_sync_controller
pub fn spawn_loro_org_sync(
    root_directory: PathBuf,
    backend: Arc<LoroBackend>,
    doc_store: Arc<tokio::sync::RwLock<holon::sync::LoroDocumentStore>>,
    fs: Arc<dyn holon_filesystem::FileSystem>,
    change_source: Arc<dyn holon_filesystem::FileChangeSource>,
) -> (
    tokio::sync::watch::Receiver<Option<Result<(), String>>>,
    Arc<holon_orgmode::OrgSyncIdleSignal>,
) {
    use holon_orgmode::di::{run_file_sync_controller, LoroAliasRegistrar, OrgRerender};
    use holon_orgmode::file_sync_controller::FileSyncController;

    // Canonicalize the root the same way `OrgModeConfig::new` does:
    // `FileSyncController` canonicalizes its internal root, and `on_file_changed`
    // strip_prefixes scanned paths against it. macOS `/tmp` → `/private/tmp`, so
    // a raw root makes `scan_org_files` yield paths that fail the prefix check.
    let root_directory = fs.canonicalize(&root_directory).unwrap_or(root_directory);
    let block_reader: Arc<dyn BlockReader> = Arc::new(LoroBlockReader::new(backend.clone()));
    let doc_manager: Arc<dyn DocumentManager> = Arc::new(LoroDocumentManager::new(backend.clone()));
    let ordering: Arc<dyn BlockOrdering> = Arc::new(LoroBlockOrdering::new(backend.clone()));

    let controller = FileSyncController::new(
        block_reader,
        doc_manager,
        root_directory.clone(),
        ordering,
        fs.clone(),
    )
    .with_alias_registrar(Arc::new(LoroAliasRegistrar {
        doc_store: doc_store.clone(),
    }));

    let (ready_sender, ready_signal) = holon_orgmode::FileWatcherReadySignal::new();
    let ready_sender_arc = Arc::new(std::sync::Mutex::new(Some(ready_sender)));

    let idle = holon_orgmode::OrgSyncIdleSignal::new();
    let idle_weak = Arc::downgrade(&idle);

    // No block→org re-render in the no-Turso path (out of scope for this task).
    // Dropping `_tx` closes the channel so the `rerender_rx.recv()` select arm
    // stays dormant.
    let (_tx, rerender_rx) = tokio::sync::mpsc::unbounded_channel::<OrgRerender>();

    tokio::spawn(run_file_sync_controller(
        controller,
        root_directory,
        idle_weak,
        rerender_rx,
        ready_sender_arc,
        fs,
        change_source,
    ));

    (ready_signal.into_receiver(), idle)
}

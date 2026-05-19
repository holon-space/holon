//! Unified bidirectional sync controller for Org files ↔ block store.
//!
//! Unified bidirectional sync: a single component
//! that uses the **projection + diff-ingestion** pattern:
//!
//! - `last_projection`: what we last wrote to (or confirmed on) disk, per file.
//! - Echo suppression: `disk_content == last_projection[file]` (no timing window).
//! - External edits: detected by diffing against `last_projection`.
//!
//! The controller runs on a single task via `tokio::select!`, so `on_file_changed`
//! and `on_block_changed` are serialized — no concurrent access to `last_projection`.
//!
//! **Decoupled from Loro/Turso**: uses `BlockReader` and `DocumentManager` traits.

use anyhow::{Context, Result};
use holon::core::datasource::OperationProvider;
use holon::sync::event_bus::EventOrigin;
use holon::sync::CanonicalPath;
use holon_api::block::Block;
use holon_api::{EntityName, EntityUri, Value};
use holon_core::file_format::FileFormatAdapter;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

use holon_core::block_ordering::BlockOrdering;

use crate::block_params::build_block_params;
use crate::file_format::OrgFormatAdapter;
use crate::models::{OrgBlockExt, OrgDocumentExt};
use crate::parser::parse_doc_id;
use crate::traits::{BlockReader, DocumentManager, ImageDataProvider};

/// Bump when the org renderer changes in a way that alters the canonical
/// projection bytes (formatting, property ordering, directive layout, …).
/// Mismatch on next boot forces a one-shot re-ingest per file so the stored
/// `file.content_hash` snaps to the new canonical form.
pub const RENDERER_VERSION: &str = "1";

pub struct OrgSyncController {
    /// What we last wrote to (or confirmed on) disk, per file.
    /// Uses CanonicalPath to resolve macOS /var → /private/var symlinks,
    /// so scan_org_files and file watcher events match the same key.
    /// Session-only, populated lazily on first miss.
    last_projection: HashMap<CanonicalPath, String>,

    /// Phase 1 fast-path: `sha256(RENDERER_VERSION || render(parsed_blocks))`
    /// per file, persisted via `file.content_hash`. Populated at startup
    /// from `block_reader.load_file_hashes()` so a cold boot of an unchanged
    /// vault skips block-table batches entirely (parses + renders + hashes,
    /// then compares — no SQL writes when the hash matches).
    last_projection_hash: HashMap<CanonicalPath, String>,

    /// Cheap dirty-check signature `(mtime, size)` per tracked path. Used by
    /// `poll_external_changes` to skip the expensive `read_to_string` when
    /// `stat()` shows the file hasn't changed since we last looked. Updated
    /// after every read so subsequent polls compare against the post-read
    /// state. A missing entry forces a full read (treats the path as dirty).
    disk_signatures: HashMap<CanonicalPath, (std::time::SystemTime, u64)>,

    /// Reads blocks by document ID.
    block_reader: Arc<dyn BlockReader>,

    /// Command bus for writing blocks (always SqlOperationProvider for read/write consistency).
    command_bus: Arc<dyn OperationProvider>,

    /// Document entity CRUD (decoupled from Turso).
    doc_manager: Arc<dyn DocumentManager>,

    /// Root directory for org files.
    root_dir: PathBuf,

    /// Callback to register doc_id → path aliases in the storage layer.
    /// Set by the DI wiring when Loro is available.
    alias_registrar: Option<Arc<dyn AliasRegistrar>>,

    /// Shell command to run after each org file write (from holon.toml).
    post_org_write_hook: Option<String>,

    /// Binary image data provider (Loro-backed). Used to materialize image
    /// files to disk on render and ingest them from disk on parse.
    image_data: Option<Arc<dyn ImageDataProvider>>,

    /// File format adapter — delegates parse/render so the controller works
    /// across formats. Defaults to `OrgFormatAdapter`; future markdown /
    /// notion / logseq adapters plug in here without changing the
    /// controller's logic.
    format: Arc<dyn FileFormatAdapter>,

    /// Positional-intent writer. Used during disk-order replay to move
    /// misaligned blocks into the position recorded in the org file.
    ordering: Arc<dyn BlockOrdering>,
}

/// Callback for registering doc_id → path aliases in the storage layer.
/// Implemented by Loro wiring; the controller itself doesn't know about Loro.
#[async_trait::async_trait]
pub trait AliasRegistrar: Send + Sync {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path);
    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf>;
}

impl OrgSyncController {
    pub fn new(
        block_reader: Arc<dyn BlockReader>,
        command_bus: Arc<dyn OperationProvider>,
        doc_manager: Arc<dyn DocumentManager>,
        root_dir: PathBuf,
        ordering: Arc<dyn BlockOrdering>,
    ) -> Self {
        Self::with_format(
            block_reader,
            command_bus,
            doc_manager,
            root_dir,
            Arc::new(OrgFormatAdapter::new()),
            ordering,
        )
    }

    /// Construct a controller with an explicit `FileFormatAdapter`. The
    /// `new` constructor uses `OrgFormatAdapter`; tests and future markdown /
    /// notion / logseq wirings call this directly.
    pub fn with_format(
        block_reader: Arc<dyn BlockReader>,
        command_bus: Arc<dyn OperationProvider>,
        doc_manager: Arc<dyn DocumentManager>,
        root_dir: PathBuf,
        format: Arc<dyn FileFormatAdapter>,
        ordering: Arc<dyn BlockOrdering>,
    ) -> Self {
        // Canonicalize root_dir so strip_prefix works with canonical file paths
        // (macOS: /var → /private/var symlink resolution).
        let root_dir = CanonicalPath::new(&root_dir).into_path_buf();
        Self {
            last_projection: HashMap::new(),
            last_projection_hash: HashMap::new(),
            disk_signatures: HashMap::new(),
            block_reader,
            command_bus,
            doc_manager,
            root_dir,
            alias_registrar: None,
            post_org_write_hook: None,
            image_data: None,
            format,
            ordering,
        }
    }

    pub fn with_alias_registrar(mut self, registrar: Arc<dyn AliasRegistrar>) -> Self {
        self.alias_registrar = Some(registrar);
        self
    }

    pub fn with_post_org_write_hook(mut self, cmd: String) -> Self {
        self.post_org_write_hook = Some(cmd);
        self
    }

    pub fn with_image_data(mut self, provider: Arc<dyn ImageDataProvider>) -> Self {
        self.image_data = Some(provider);
        self
    }

    /// Initialize last_projection from the block reader's current state.
    ///
    /// Must be called at startup BEFORE scanning files, so that we have a
    /// diff base for detecting external edits.
    pub async fn initialize(&mut self) -> Result<()> {
        // Phase 1 fast-path: load persisted `(file_id, content_hash)` pairs
        // from the `file` table BEFORE the in-process cache has replayed file
        // events. If an on-disk file's `hash(RENDERER_VERSION || disk_bytes)`
        // matches its stored hash, `on_file_changed` skips block ingest
        // entirely — the dominant cold-boot cost. See plan §Phase 1.
        match self.block_reader.load_file_hashes().await {
            Ok(rows) => {
                for (uri, hash) in rows {
                    if let Some(canonical) = self.file_uri_to_canonical_path(&uri) {
                        self.last_projection_hash.insert(canonical, hash);
                    }
                }
                info!(
                    "[OrgSyncController] Loaded last_projection_hash for {} files \
                     (will skip ingest when disk_bytes hash matches)",
                    self.last_projection_hash.len()
                );
            }
            Err(e) => {
                warn!(
                    "[OrgSyncController] load_file_hashes failed; cold-boot fast \
                     path disabled, will re-ingest every file. Error: {e}"
                );
            }
        }

        // last_projection (full rendered string) is intentionally NOT eagerly
        // populated by walking every block — it's a session-only cache used
        // for echo suppression, populated lazily on first miss by
        // `on_file_changed`. Walking iter_documents_with_blocks here would
        // pay parse+render cost for every doc on every boot.
        info!("[OrgSyncController] Initialize complete");
        Ok(())
    }

    /// Convert a `file:<encoded-path>` EntityUri back to a CanonicalPath
    /// relative to this controller's `root_dir`. Returns None if the URI
    /// scheme isn't `file:` or the on-disk path doesn't exist (which can
    /// legitimately happen if the user deleted the file while the row
    /// lingers — next sync will tombstone it).
    fn file_uri_to_canonical_path(&self, uri: &EntityUri) -> Option<CanonicalPath> {
        if uri.scheme() != "file" {
            return None;
        }
        let encoded = uri.id();
        // `EntityUri::file` percent-encodes path segments; reverse it before
        // joining with root_dir so spaces etc. match the on-disk file name.
        // `decode_utf8_lossy` substitutes U+FFFD for invalid sequences rather
        // than swallowing them — keeps the fast-path correct for ASCII paths
        // and visibly broken for the rare non-UTF-8 case.
        let decoded = percent_encoding::percent_decode_str(encoded).decode_utf8_lossy();
        let abs = self.root_dir.join(decoded.as_ref());
        Some(CanonicalPath::new(&abs))
    }

    /// Phase 1: `sha256(RENDERER_VERSION || disk_bytes)`. Same hash function
    /// is used both to gate ingest on read and to stamp `file.content_hash`
    /// after write so the next boot's gate compares like-for-like.
    fn projection_hash(disk_bytes: &str) -> String {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(RENDERER_VERSION.as_bytes());
        hasher.update(b"\0");
        hasher.update(disk_bytes.as_bytes());
        hex::encode(hasher.finalize())
    }

    /// Handle a file change event from the FileWatcher.
    ///
    /// Echo suppression: if disk content matches last_projection, skip.
    /// Otherwise, diff against last_projection to compute create/update/delete ops.
    #[tracing::instrument(skip(self), name = "org.on_file_changed", fields(path = %path.display()))]
    pub async fn on_file_changed(&mut self, path: &Path) -> Result<()> {
        let canonical = CanonicalPath::new(path);
        let disk_content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                debug!("[OrgSyncController] File deleted: {}", path.display(),);
                return Ok(());
            }
            Err(e) => {
                return Err(e).with_context(|| {
                    format!("[OrgSyncController] Cannot read {}", path.display())
                });
            }
        };

        let last = self
            .last_projection
            .get(&canonical)
            .map(|s| s.as_str())
            .unwrap_or("");

        tracing::debug!(
            "[ORGSYNC_ENTER] {} disk_len={} last_len={} has_key={} equal={}",
            path.display(),
            disk_content.len(),
            last.len(),
            self.last_projection.contains_key(&canonical),
            disk_content == last,
        );

        // Echo suppression: skip if we have a prior projection and content matches.
        // An absent entry means "first time seeing this file" — always process it
        // to create the document entity (needed for block→file sync).
        if self.last_projection.contains_key(&canonical) && disk_content == last {
            debug!(
                "[OrgSyncController] Skipping {} — matches last_projection",
                path.display()
            );
            return Ok(());
        }

        // Phase 1 cold-boot fast-path: when `last_projection` has no entry
        // (first time we see this file this session) but `last_projection_hash`
        // — loaded from `file.content_hash` at startup — matches the disk
        // bytes hashed with the same renderer-version-prefixed sha256, the
        // ingest path is a guaranteed no-op (we wrote this content last time
        // and nothing changed on disk). Skip ingest entirely; stamp
        // `last_projection` so subsequent in-session echo-suppression hits.
        //
        // Approach A (disk-bytes hash, not projection hash): the false-miss
        // case (user externally edited in a benign way — trailing newline,
        // property reorder — that re-renders to the same projection) costs
        // exactly one parse + diff + zero block ops (Phase 2 ensures the
        // edge sets don't churn either), then re-stamps the hash. Bounded
        // and only fires on actual edits. Approach B (projection hash) would
        // parse + render every file on every boot to confirm "skip" — a
        // guaranteed cost per boot we don't pay here.
        let disk_hash = Self::projection_hash(&disk_content);
        if let Some(stored) = self.last_projection_hash.get(&canonical) {
            if stored == &disk_hash {
                debug!(
                    "[OrgSyncController] Skipping {} — disk hash matches \
                     stored file.content_hash (cold-boot fast path)",
                    path.display()
                );
                self.last_projection.insert(canonical.clone(), disk_content);
                return Ok(());
            }
        }

        info!(
            "[OrgSyncController] Processing external change: {}",
            path.display()
        );

        let rel_path = path.strip_prefix(&self.root_dir).map_err(|e| {
            anyhow::anyhow!(
                "File {} not under root {}: {}",
                path.display(),
                self.root_dir.display(),
                e
            )
        })?;

        // Resolve the document entity. `#+ID: <bare>` (when present) is the
        // authoritative identity — it survives renames. When absent we fall
        // back to name-chain resolution and emit `#+ID:` on the next render
        // so subsequent loads pick up the stable identity from the file.
        let bare_id_in_file = parse_doc_id(&disk_content);
        let segments = path_to_name_chain(rel_path);
        let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
        let document = match bare_id_in_file.as_deref() {
            Some(bare) => {
                let id = EntityUri::block(bare);
                match self.doc_manager.get_by_id(&id).await? {
                    Some(doc) => doc,
                    None => {
                        let title = segments
                            .last()
                            .cloned()
                            .unwrap_or_else(|| "unknown".to_string());
                        let parent_id = if segments.len() > 1 {
                            let parent_segments: Vec<&str> =
                                segment_refs[..segments.len() - 1].to_vec();
                            self.doc_manager
                                .get_or_create_by_name_chain(&parent_segments)
                                .await?
                                .id
                        } else {
                            EntityUri::no_parent()
                        };
                        let mut new_doc = Block::new_text(id, parent_id, title);
                        new_doc.set_page(true);
                        self.doc_manager.create(new_doc).await?
                    }
                }
            }
            None => {
                self.doc_manager
                    .get_or_create_by_name_chain(&segment_refs)
                    .await?
            }
        };
        let document_uri = document.id.clone();

        // Register UUID → file path alias (if Loro is available)
        if let Some(ref registrar) = self.alias_registrar {
            registrar.register_alias(&document_uri, path).await;
        }

        // Parse old state: from last_projection, or from DB on first run.
        // On first run (no last_projection), the DB may already have blocks
        // (e.g. from seed_default_layout). Querying the DB ensures these
        // existing blocks are treated as "updates" (not "creates"), so the
        // org file content correctly overwrites seed data.
        let old_blocks = if last.is_empty() {
            self.block_reader
                .get_blocks(&document_uri)
                .await
                .unwrap_or_default()
                .into_iter()
                .map(|b| (b.id.clone(), b))
                .collect()
        } else {
            match self
                .format
                .parse(path, last, &EntityUri::no_parent(), &self.root_dir)
            {
                Ok(result) => result
                    .blocks
                    .into_iter()
                    .map(|b| (b.id.clone(), b))
                    .collect(),
                Err(_) => HashMap::new(),
            }
        };

        let new_parse =
            self.format
                .parse(path, &disk_content, &EntityUri::no_parent(), &self.root_dir)?;

        // Sync #+TODO: keywords from the parsed file to the document block.
        // The parser extracts these from the file header, but the document entity
        // (created via DocumentManager) doesn't carry them. Without this, re-renders
        // via render_document() omit the #+TODO: header.
        let parsed_kws = new_parse.document.todo_keywords();
        let existing_kws = document.todo_keywords();
        if parsed_kws != existing_kws {
            let mut doc = document;
            doc.set_todo_keywords(parsed_kws);
            self.doc_manager.update_metadata(&doc).await?;
        }

        let new_blocks_vec = new_parse.blocks;
        let new_blocks: HashMap<EntityUri, Block> = new_blocks_vec
            .iter()
            .map(|b| (b.id.clone(), b.clone()))
            .collect();

        // Check for duplicate block IDs owned by other documents
        let new_block_ids: Vec<EntityUri> = new_blocks_vec
            .iter()
            .filter(|b| !old_blocks.contains_key(&b.id))
            .map(|b| b.id.clone())
            .collect();
        let conflicts = self
            .block_reader
            .find_foreign_blocks(&new_block_ids, &document_uri)
            .await?;
        let conflict_ids: std::collections::HashSet<EntityUri> =
            conflicts.iter().map(|(id, _)| id.clone()).collect();
        if !conflicts.is_empty() {
            info!(
                "[OrgSyncController] Re-parenting {} blocks from other documents to {} \
                 (blocks exist under different doc URIs, e.g. from seed_default_layout). \
                 File: {}",
                conflicts.len(),
                document_uri,
                path.display(),
            );
        }

        // Collect all block operations into a batch
        let mut operations: Vec<(String, HashMap<String, Value>)> = Vec::new();
        let mut has_structural_changes = false;
        let mut created_ids: Vec<String> = Vec::new();
        let mut updated_via_conflict_ids: Vec<String> = Vec::new();

        // Creates (in document order so parents before children).
        // Blocks that already exist under a different document are re-parented
        // via "update" instead of "create" (INSERT OR IGNORE would silently skip them).
        //
        // For each new block we attach the typed positional intent
        // `after_block_id = <previous sibling in file under same parent>`,
        // tracked in `last_block_per_parent` as we walk `new_blocks_vec`
        // (which is in DFS document order). The predecessor may be an
        // existing block (already in old_blocks, already in Loro) or a
        // freshly-created block earlier in this batch — both work, because
        // `LoroSyncController` processes Created events serially, so the
        // predecessor is in the tree by the time `apply_create` resolves
        // the position.
        //
        // Without this, the inbound CDC path fell back to a sort_key
        // sibling-scan that compared the org parser's `gen_n_keys` values
        // against Loro's auto-assigned fractional indices — two
        // generation strategies that don't share a value space, so the
        // scan picked the wrong predecessor (or none) and collapsed
        // children to the front of the list. See Phase 3.7 / the Stage 2
        // cleanup devlog for the empirical confirmation.
        // Walk in document order, tracking the predecessor under each parent
        // as we go. Existing blocks anchor the position of subsequent new
        // siblings, so the cursor advances for every block — not just the
        // new ones. Records each block's predecessor (or `None` for "first
        // child") in `predecessors`; both the creates pass below and the
        // updates pass further down look it up to attach the typed
        // `after_block_id` param.
        let mut last_block_per_parent: HashMap<EntityUri, EntityUri> = HashMap::new();
        let mut predecessors: HashMap<EntityUri, Option<EntityUri>> = HashMap::new();
        for block in &new_blocks_vec {
            let parent_id = if block.parent_id == new_parse.document.id {
                &document_uri
            } else {
                &block.parent_id
            };
            let pred = last_block_per_parent.get(parent_id).cloned();
            predecessors.insert(block.id.clone(), pred);
            last_block_per_parent.insert(parent_id.clone(), block.id.clone());
        }

        // Creates pass.
        for block in &new_blocks_vec {
            if !old_blocks.contains_key(&block.id) {
                let parent_id = if block.parent_id == new_parse.document.id {
                    &document_uri
                } else {
                    &block.parent_id
                };
                let mut params = build_block_params(block, parent_id, &document_uri);
                if let Some(Some(prev)) = predecessors.get(&block.id) {
                    params.insert(
                        holon::sync::event_bus::POSITION_AFTER_BLOCK_ID_PARAM.to_string(),
                        Value::String(prev.to_string()),
                    );
                }
                let op = if conflict_ids.contains(&block.id) {
                    "update"
                } else {
                    "create"
                };
                if op == "create" {
                    has_structural_changes = true;
                    created_ids.push(block.id.to_string());
                } else {
                    updated_via_conflict_ids.push(block.id.to_string());
                }
                operations.push((op.to_string(), params));
            }
        }
        tracing::debug!(
            "[ORGSYNC_DIFF] {} old={} new={} creates={} conflict_updates={} creates_ids={:?}",
            path.display(),
            old_blocks.len(),
            new_blocks_vec.len(),
            created_ids.len(),
            updated_via_conflict_ids.len(),
            created_ids,
        );

        // Updates pass. Existing blocks may have moved within their parent's
        // children list (e.g. when a 2nd BulkExternalAdd grows the sibling
        // set, every sibling's `gen_n_keys`-assigned sort_key gets
        // re-canonicalised). Inject the typed `after_block_id` here too so
        // `apply_update_with_backend` can `tree.mov_after` against the
        // file's predecessor instead of relying on the sort_key sibling-scan
        // — same gen-strategy-mismatch concern as creates.
        //
        // Iterate `new_blocks_vec` (document order), NOT `new_blocks`
        // (HashMap, non-deterministic). Update events get applied
        // sequentially by `LoroSyncController`, and each `tree.mov_after`
        // depends on the *current* tree state at apply time. If updates
        // arrived in HashMap order, a later sibling could mov_after its
        // predecessor *before* the predecessor itself had been moved,
        // scrambling the children list.
        for new_block in &new_blocks_vec {
            let id = &new_block.id;
            if let Some(old_block) = old_blocks.get(id) {
                if blocks_differ(old_block, new_block) {
                    let parent_id = if new_block.parent_id == new_parse.document.id {
                        &document_uri
                    } else {
                        &new_block.parent_id
                    };
                    let mut params = build_block_params(new_block, parent_id, &document_uri);
                    if let Some(Some(prev)) = predecessors.get(id) {
                        params.insert(
                            holon::sync::event_bus::POSITION_AFTER_BLOCK_ID_PARAM.to_string(),
                            Value::String(prev.to_string()),
                        );
                    }
                    // Phase 2: drop edge fields from params when unchanged, so
                    // SqlOperationProvider's edge_field_replace_sql (DELETE +
                    // re-INSERT into block_requires/block_tags) is not invoked.
                    // Junction values are order-undefined on read, so compare as
                    // sets. Idempotent re-ingests of an unchanged vault stop
                    // churning ~2,400 statements per startup.
                    strip_unchanged_edge_fields(&mut params, old_block, new_block);
                    operations.push(("update".to_string(), params));
                }
            }
        }

        // Deletes
        for id in old_blocks.keys() {
            if !new_blocks.contains_key(id) {
                has_structural_changes = true;
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.to_string()));
                // Phase 3: pin the document URI so the provider's prepare_delete
                // skips the WITH RECURSIVE Page-walk (find_document_uri).
                params.insert(
                    holon::sync::event_bus::ROUTING_DOC_URI_KEY.to_string(),
                    Value::String(document_uri.to_string()),
                );
                operations.push(("delete".to_string(), params));
            }
        }

        // Execute all operations as a single batch (one transaction + one event batch).
        // Tagged `EventOrigin::Org` so `LoroSyncController`'s inbound runtime gate
        // (Phase 3.3 step 2) recognises file-watcher reflections as a legitimate
        // post-startup SQL→Loro source and lets them through after the gate is
        // flipped off. Untagged ("sql") events would be dropped as suspected
        // chord-op SQL-direct writes.
        let expected_block_count = new_blocks.len();
        if !operations.is_empty() {
            self.command_bus
                .execute_batch_with_origin(&EntityName::new("block"), operations, EventOrigin::Org)
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Batch block operations failed for {}: {}",
                        path.display(),
                        e
                    )
                })?;

            // Phase 4: wait for the CDC-driven cache to reflect the committed
            // changes via the ack-watermark signal (push-based on CDC), not
            // a 10 ms `SELECT * FROM block` full-scan poll. Capture
            // `target_ts` AFTER the batch returns so events.created_at <=
            // target_ts for every event we just published. The cache
            // consumer acks in order, so its watermark reaching target_ts
            // proves our batch has been fully applied.
            //
            // V-4a caveat: `Consumer::CACHE` is shared by block/directory/
            // file cache subscribers. In rare cases a non-block ack could
            // race ahead of the block one; the sanity SELECT below catches
            // that. A timeout still falls back to the strict get_blocks
            // count check to preserve loud failure on a stuck consumer.
            let target_ts = chrono::Utc::now().timestamp_millis();
            let caught_up = self
                .block_reader
                .wait_for_cache_caught_up(target_ts, 2000)
                .await?;
            let cached_blocks = self.block_reader.get_blocks(&document_uri).await?;
            if cached_blocks.len() < expected_block_count {
                anyhow::bail!(
                    "[on_file_changed] CDC cache did not catch up within 2s for {} \
                     (expected {} blocks, cache has {}, watermark_caught_up={})",
                    path.display(),
                    expected_block_count,
                    cached_blocks.len(),
                    caught_up
                );
            }
        }

        // Disk-order replay: move any block that is not already in the position
        // recorded in the parsed org file. One `children()` call per distinct
        // parent (cached in `live_children`), O(N) total reads.
        //
        // Before reading children we wait for every newly-created block to be
        // visible to the ordering layer. `execute_batch_with_origin` above
        // published `EventOrigin::Org` create events whose Loro-side
        // application is asynchronous (the `LoroSyncController` inbound
        // consumer processes them off the EventBus). The CDC-cache wait at
        // ~line 528 only gates on the SQL projection; if we proceed straight
        // to `ordering.place` we may call `update_block_position` for a
        // block whose Loro tree node hasn't been created yet, surfacing as
        // `Block not found: <id>` — the block then lands at Loro's default
        // position and the renderer's children-of-doc query never finds it.
        // Polling `ordering.children(parent)` reads through the same path
        // `ordering.place` will use, so once a created id appears there the
        // subsequent `place` is guaranteed to find it.
        {
            let mut live_children: HashMap<String, Vec<String>> = HashMap::new();
            let mut expected_per_parent: HashMap<String, HashSet<String>> = HashMap::new();
            // `BlockOrdering::children` filters `b.parent_id.as_str() == parent_id`,
            // and `EntityUri::as_str()` returns the FULL URI (`"block:ref-doc-0"`).
            // Keys here are full URIs so the compare matches.
            for new_block in &new_blocks_vec {
                if !created_ids.contains(&new_block.id.to_string()) {
                    continue;
                }
                let parent_key = if new_block.parent_id == new_parse.document.id {
                    document_uri.as_str().to_string()
                } else {
                    new_block.parent_id.as_str().to_string()
                };
                // Compare against the full URI form returned by
                // `BlockOrdering::children` (kids are `b.id.as_str()`).
                expected_per_parent
                    .entry(parent_key)
                    .or_default()
                    .insert(new_block.id.as_str().to_string());
            }
            let propagate_deadline =
                tokio::time::Instant::now() + tokio::time::Duration::from_millis(2000);
            for (parent_key, expected_ids) in &expected_per_parent {
                loop {
                    let kids = self
                        .ordering
                        .children(parent_key)
                        .await
                        .map_err(|e| anyhow::anyhow!("ordering.children failed: {e}"))?;
                    let present: HashSet<&str> = kids.iter().map(String::as_str).collect();
                    if expected_ids.iter().all(|id| present.contains(id.as_str())) {
                        live_children.insert(parent_key.clone(), kids);
                        break;
                    }
                    if tokio::time::Instant::now() >= propagate_deadline {
                        let missing: Vec<&String> = expected_ids
                            .iter()
                            .filter(|id| !present.contains(id.as_str()))
                            .collect();
                        anyhow::bail!(
                            "[on_file_changed] new blocks did not appear in ordering for \
                             parent {parent_key} within 2s: missing {missing:?}; \
                             present children: {kids:?}"
                        );
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
                }
            }
            // Backfill children lists for parents that only contained
            // pre-existing blocks (no creates) — the wait loop above skipped
            // them entirely.
            for new_block in &new_blocks_vec {
                let parent_key = if new_block.parent_id == new_parse.document.id {
                    document_uri.as_str().to_string()
                } else {
                    new_block.parent_id.as_str().to_string()
                };
                #[allow(clippy::map_entry)]
                // async fetch between check + insert, entry API doesn't fit
                if !live_children.contains_key(&parent_key) {
                    let kids = self
                        .ordering
                        .children(&parent_key)
                        .await
                        .map_err(|e| anyhow::anyhow!("ordering.children failed: {e}"))?;
                    live_children.insert(parent_key, kids);
                }
            }

            for new_block in &new_blocks_vec {
                // Source / image children are grouped ahead of text by
                // `OrgRenderer::render_entity_tree` regardless of sort_key
                // (see assertions.rs `render_group`). They also don't land
                // in the Loro tree — synthetic ids like `<parent>::render::0`
                // exist only in SQL — so `place()` would surface
                // `Block not found` through `update_block_position`.
                if !matches!(new_block.content_type, holon_api::ContentType::Text) {
                    continue;
                }
                let parent = if new_block.parent_id == new_parse.document.id {
                    &document_uri
                } else {
                    &new_block.parent_id
                };
                // Full-URI form throughout: `BlockOrdering::children` /
                // `prev_sibling` return `b.id.as_str()` = `"block:foo"`, and
                // `place()`'s internal comparisons (sql_block_operations.rs:182)
                // also use `as_str()`. Mixing bare ids here silently skips
                // every block.
                let want_after: Option<&str> = predecessors
                    .get(&new_block.id)
                    .and_then(|p| p.as_ref())
                    .map(|u| u.as_str());

                let siblings = live_children
                    .get(parent.as_str())
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                let Some(current_idx) = siblings.iter().position(|s| s == new_block.id.as_str())
                else {
                    // Wait-loop above guarantees newly-created blocks are
                    // present in `live_children`; pre-existing blocks were
                    // backfilled. Anything missing here is a real bug.
                    anyhow::bail!(
                        "[on_file_changed] block {} not found in live_children under {}: {:?}",
                        new_block.id.as_str(),
                        parent.as_str(),
                        siblings
                    );
                };
                let current_prev = if current_idx == 0 {
                    None
                } else {
                    siblings.get(current_idx - 1).map(String::as_str)
                };
                if current_prev == want_after {
                    continue;
                }

                self.ordering
                    .place(&new_block.id, parent.as_str(), want_after)
                    .await
                    .map_err(|e| anyhow::anyhow!("ordering.place failed: {e}"))?;
            }
        }

        // Ingest image files from disk into the image data provider (if any).
        // At this point blocks are in the store and image files are on disk.
        self.ingest_images(&document_uri).await?;

        // For UPDATE-only ingestion (no creates/deletes), the disk content already
        // reflects the authoritative state — we just parsed it and persisted the
        // diff to SQL. Re-rendering from the CDC cache here would be racy: count-
        // based waiting can't detect property updates, so the cache may still
        // return the pre-update row and we'd overwrite the file with stale data,
        // losing the properties we just ingested. Skip the round-trip entirely
        // and record the disk content as the new projection.
        //
        // EXCEPTION: when the file lacks a `#+ID:` directive, force the round-trip
        // so the renderer can persist `#+ID: <uuid>` to disk. This makes the
        // document's identity rename-safe and lets future loads short-circuit the
        // name-chain lookup.
        let needs_id_writeback = bare_id_in_file.is_none();
        if !has_structural_changes && !needs_id_writeback {
            self.last_projection
                .insert(canonical.clone(), disk_content.to_string());
            self.persist_disk_hash_for(&canonical, rel_path, &disk_hash)
                .await;
            return Ok(());
        }

        // Structural changes occurred — re-project from cache so the file reflects
        // any merges (e.g. conflict re-parenting, seed layout integration).
        let rendered = self.render_file_by_doc_id(&document_uri, path).await?;
        assert!(
            new_blocks.is_empty() || !rendered.trim().is_empty(),
            "[OrgSyncController] BUG: Just created/updated {} blocks for doc_id={} \
             but render_file_by_doc_id returned empty for {}. \
             This would wipe the file!",
            new_blocks.len(),
            document_uri,
            path.display(),
        );

        if rendered != disk_content {
            // TOCTOU guard: re-read the disk NOW. If it changed since we parsed
            // it, a concurrent external write has landed new content — writing
            // `rendered` (derived from a stale CDC cache) would wipe that
            // external write off disk. Defer to the next on_file_changed
            // invocation (FSEvents and the poll backstop will both fire for the
            // new disk content), and stamp `last_projection` with the version
            // we reconciled so the next diff sees the true external delta.
            match tokio::fs::read_to_string(path).await {
                Ok(now) if now != disk_content => {
                    tracing::debug!(
                        "[ORGSYNC_TOCTOU] {} disk changed during processing \
                         (parsed_len={} disk_now_len={}); skipping write-back, \
                         stamping last_projection with parsed content so next \
                         diff picks up the external delta.",
                        path.display(),
                        disk_content.len(),
                        now.len(),
                    );
                    self.last_projection.insert(canonical.clone(), disk_content);
                    return Ok(());
                }
                Ok(_) => {
                    if let Some(parent) = path.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::write(path, &rendered).await?;
                    self.run_post_write_hook(path);
                    info!(
                        "[OrgSyncController] Wrote merged content to {}",
                        path.display()
                    );
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    // File deleted since we parsed it. Nothing to do.
                    return Ok(());
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!(
                            "[OrgSyncController] TOCTOU re-read failed for {}",
                            path.display()
                        )
                    });
                }
            }
        }

        // Phase 1: stamp file.content_hash for the bytes that are NOW on
        // disk (= `rendered` if we wrote it, else still == disk_content;
        // in both cases `rendered` is the canonical projection). Updates
        // in-memory map and persists to SQL so next boot's fast-path engages.
        let final_hash = Self::projection_hash(&rendered);
        self.persist_disk_hash_for(&canonical, rel_path, &final_hash)
            .await;

        // Update last_projection
        self.last_projection.insert(canonical.clone(), rendered);

        Ok(())
    }

    /// Update `last_projection_hash` in memory and persist to
    /// `file.content_hash` via the BlockReader's raw-SQL write-back. Best-
    /// effort: a failure to persist (e.g. file row not yet created by
    /// `OrgmodeSyncProvider`) does not abort the ingest — we've already
    /// committed the block ops and don't want to bail the controller. The
    /// next sync will create the row and the following boot will write
    /// the hash successfully. Logged at warn so the case is observable.
    async fn persist_disk_hash_for(
        &mut self,
        canonical: &CanonicalPath,
        rel_path: &Path,
        hash: &str,
    ) {
        self.last_projection_hash
            .insert(canonical.clone(), hash.to_string());
        let rel = rel_path.to_string_lossy();
        let file_uri = EntityUri::file(&rel);
        if let Err(e) = self.block_reader.persist_file_hash(&file_uri, hash).await {
            warn!(
                "[OrgSyncController] persist_file_hash failed for {} ({}): {} \
                 (in-memory hash updated; next boot will re-ingest)",
                file_uri,
                canonical.as_path_buf().display(),
                e
            );
        }
    }

    /// Handle a block change notification (from EventBus or Loro).
    ///
    /// Re-renders the affected file and writes if content changed.
    /// Returns `true` if a matching document file was found and re-rendered,
    /// `false` if the doc_id didn't map to any known file.
    #[tracing::instrument(skip(self), name = "org.on_block_changed", fields(doc_id = %doc_id))]
    pub async fn on_block_changed(&mut self, doc_id: &EntityUri) -> Result<bool> {
        let path = match self.doc_id_to_path(doc_id).await {
            Some(p) => p,
            None => return Ok(false),
        };
        let canonical = CanonicalPath::new(&path);

        // If disk content differs from last_projection, there's a pending external
        // change that the file watcher hasn't delivered yet. Ingest it first so
        // the re-render below includes both the block event and the external edit.
        //
        // Only treat this as a pending external change when we have a baseline
        // (`last_projection` already holds the file). Without a baseline,
        // `last == ""` would always differ from any non-empty disk content and
        // we'd incorrectly re-ingest the on-disk file — which can revert the
        // user's just-issued UPDATE if the file watcher hasn't yet delivered the
        // initial WriteOrgFile event. The watcher will catch up on its own.
        let disk_content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let last = self
            .last_projection
            .get(&canonical)
            .map(|s| s.as_str())
            .unwrap_or("");
        if self.last_projection.contains_key(&canonical) && disk_content != last {
            info!(
                "[OrgSyncController] Processing pending external change for {} before re-render",
                path.display()
            );
            self.on_file_changed(&path).await?;
        }

        let rendered = self.render_file_by_doc_id(doc_id, &path).await?;

        let current_last = self
            .last_projection
            .get(&canonical)
            .map(|s| s.as_str())
            .unwrap_or("");

        if rendered == current_last {
            return Ok(true);
        }

        // TOCTOU guard: disk may have changed again since we read it above
        // (concurrent external write). Writing `rendered` here — derived
        // from the CDC cache which may lag behind the new disk content —
        // would wipe the external write. Re-read and bail if changed.
        let disk_at_write = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        if disk_at_write != disk_content {
            tracing::debug!(
                "[ORGSYNC_TOCTOU on_block_changed] {} disk changed during processing \
                 (initial_len={} disk_now_len={}); skipping write-back.",
                path.display(),
                disk_content.len(),
                disk_at_write.len(),
            );
            return Ok(true);
        }

        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(&path, &rendered).await?;
        self.run_post_write_hook(&path);
        self.materialize_images(doc_id).await?;
        self.last_projection.insert(canonical, rendered);

        info!(
            "[OrgSyncController] Wrote block changes to {}",
            path.display()
        );

        Ok(true)
    }

    /// Poll all tracked files for pending external changes that the file
    /// watcher may have missed (FSEvents on macOS can coalesce or drop
    /// events under load). For each file whose disk content differs from
    /// `last_projection`, call `on_file_changed` to ingest the edit.
    ///
    /// Called from a periodic timer in the DI sync loop as a backstop for
    /// notify-driven delivery. Returns the number of files that were
    /// ingested (0 if everything was already in sync).
    #[tracing::instrument(skip(self), name = "org.poll_external_changes")]
    pub async fn poll_external_changes(&mut self) -> Result<usize> {
        let mut ingested = self.poll_tracked_files().await?;
        ingested += self.poll_new_files().await?;
        Ok(ingested)
    }

    /// Phase A: re-check every path we already track for modifications or
    /// deletions. Echo-suppressed by `last_projection`; further short-circuited
    /// by a `(mtime, size)` signature so unchanged files don't cost a read.
    #[tracing::instrument(skip(self), name = "org.poll_tracked_files")]
    pub async fn poll_tracked_files(&mut self) -> Result<usize> {
        let mut ingested = 0;
        let keys: Vec<(CanonicalPath, PathBuf)> = self
            .last_projection
            .keys()
            .map(|k| (k.clone(), (**k).to_path_buf()))
            .collect();

        for (canonical, path) in keys {
            // Cheap dirty check: stat() the file and compare (mtime, size)
            // against the cached signature. Avoids the per-tick full-file
            // read_to_string for every tracked org file (~38 files at 10Hz
            // dominated idle CPU before this).
            let meta = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("[poll_external_changes] Cannot stat {}", path.display())
                    });
                }
            };
            let sig = (
                meta.modified().unwrap_or(std::time::SystemTime::UNIX_EPOCH),
                meta.len(),
            );
            if self.disk_signatures.get(&canonical) == Some(&sig) {
                continue;
            }

            let disk_content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("[poll_external_changes] Cannot read {}", path.display())
                    });
                }
            };
            // Stamp signature *before* the diff so the next poll skips this
            // path even if the content matches last_projection (echo) — only
            // a fresh mtime/size change re-enters the read path.
            self.disk_signatures.insert(canonical.clone(), sig);

            let last = self
                .last_projection
                .get(&canonical)
                .map(|s| s.as_str())
                .unwrap_or("");
            if disk_content != last {
                info!(
                    "[OrgSyncController] poll_external_changes: ingesting {} (disk != last_projection)",
                    path.display()
                );
                self.on_file_changed(&path).await?;
                ingested += 1;
            }
        }

        Ok(ingested)
    }

    /// Phase B: walk the tree and discover NEW files (paths not yet in
    /// `last_projection`). Backstops `notify`'s recursive watcher during its
    /// unarmed window on macOS (`notify::watch(dir, Recursive)` can take 9+s
    /// to register, leaving files created during that window invisible).
    ///
    /// Each call rebuilds `ignore::WalkBuilder` (gitignore regex DFAs), which
    /// is non-trivial — call sites should tick this much less often than the
    /// cheap `poll_tracked_files` path.
    #[tracing::instrument(skip(self), name = "org.poll_new_files")]
    pub async fn poll_new_files(&mut self) -> Result<usize> {
        let mut ingested = 0;
        let root_dir = self.root_dir.clone();
        let entries = crate::file_watcher::scan_directory(&root_dir);
        for path in entries.files {
            let canonical = CanonicalPath::new(&path);
            if !self.last_projection.contains_key(&canonical) {
                info!(
                    "[OrgSyncController] poll_new_files: discovered new file {}",
                    path.display()
                );
                self.on_file_changed(&path).await?;
                ingested += 1;
            }
        }
        Ok(ingested)
    }

    /// Re-render all tracked files (used for events where the doc_id is unknown,
    /// e.g. block.deleted, block.fields_changed).
    pub async fn re_render_all_tracked(&mut self) -> Result<()> {
        let keys: Vec<CanonicalPath> = self.last_projection.keys().cloned().collect();

        for canonical in keys {
            let path: PathBuf = (*canonical).to_path_buf();
            // If disk content differs from last_projection, ingest the pending external
            // change first so the re-render includes both the block event and external edit.
            let disk_content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    debug!("[re_render_all_tracked] File deleted: {}", path.display(),);
                    continue;
                }
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("[re_render_all_tracked] Cannot read {}", path.display())
                    });
                }
            };
            let last = self
                .last_projection
                .get(&canonical)
                .map(|s| s.as_str())
                .unwrap_or("");
            if disk_content != last {
                info!(
                    "[OrgSyncController] Processing pending external change for {} before re-render",
                    path.display()
                );
                self.on_file_changed(&path).await?;
            }

            // Resolve path → doc_id
            let rel_path = path.strip_prefix(&self.root_dir).with_context(|| {
                format!(
                    "[re_render_all_tracked] {} not under root_dir {}",
                    path.display(),
                    self.root_dir.display(),
                )
            })?;
            let segments = path_to_name_chain(rel_path);
            let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
            let doc = match self.doc_manager.find_by_name_chain(&segment_refs).await {
                Ok(Some(doc)) => doc,
                Ok(None) => {
                    // Path was tracked but no document entity exists (e.g.
                    // empty file was registered before the skip-empty guard).
                    // Downgraded to debug: re_render_all_tracked is now
                    // debounced and runs on every burst, so warn-level would
                    // flood the log on every initial scan.
                    debug!(
                        "[re_render_all_tracked] No document found for path {} (segments: {:?}) — skipping",
                        path.display(),
                        segment_refs
                    );
                    continue;
                }
                Err(e) => {
                    warn!(
                        "[re_render_all_tracked] Doc lookup error for {}: {} — skipping",
                        path.display(),
                        e
                    );
                    continue;
                }
            };

            let rendered = self.render_file_by_doc_id(&doc.id, &path).await?;

            let current_last = self
                .last_projection
                .get(&canonical)
                .map(|s| s.as_str())
                .unwrap_or("");

            if rendered == current_last {
                continue;
            }

            // TOCTOU guard: re-read disk. If it changed since we read it
            // at the top of the loop (concurrent external write), writing
            // `rendered` — derived from a potentially stale CDC cache —
            // would wipe that new content. Skip this file; the next
            // on_file_changed will pick up the external delta.
            let disk_at_write = tokio::fs::read_to_string(&path).await.unwrap_or_default();
            if disk_at_write != disk_content {
                tracing::debug!(
                    "[ORGSYNC_TOCTOU re_render_all_tracked] {} disk changed during processing \
                     (initial_len={} disk_now_len={}); skipping write-back.",
                    path.display(),
                    disk_content.len(),
                    disk_at_write.len(),
                );
                continue;
            }

            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&path, &rendered).await?;
            self.run_post_write_hook(&path);
            self.materialize_images(&doc.id).await?;
            self.last_projection.insert(canonical, rendered);

            info!("[OrgSyncController] Re-rendered {}", path.display());
        }
        Ok(())
    }

    /// Render blocks for a document by its ID.
    ///
    /// Fetches the Document to preserve file-level metadata (e.g. `#+TODO:` keywords)
    /// in the rendered output. Falls back to block-only rendering if the Document
    /// is not found.
    async fn render_file_by_doc_id(&self, doc_id: &EntityUri, path: &Path) -> Result<String> {
        let blocks = self.block_reader.get_blocks(doc_id).await?;
        let rendered = match self.doc_manager.get_by_id(doc_id).await? {
            // Use the document block's actual ID as the root parent reference,
            // since blocks have parent_id = doc.id (may differ from the doc_id
            // used for lookup, e.g. file: vs block: URI schemes).
            Some(doc) => self.format.render_document(&doc, &blocks, path, &doc.id),
            None => self.format.render_blocks(&blocks, path, doc_id),
        };
        assert!(
            blocks.is_empty() || !rendered.trim().is_empty(),
            "[render_file_by_doc_id] {} blocks from get_blocks({}) but render is empty!\n\
             Blocks: {:?}",
            blocks.len(),
            doc_id,
            blocks
                .iter()
                .map(|b| format!(
                    "{{id={}, parent_id={}, content_type={}}}",
                    b.id, b.parent_id, b.content_type
                ))
                .collect::<Vec<_>>()
        );
        Ok(rendered)
    }

    /// Write image files to disk for all image blocks in this document.
    ///
    /// Called after rendering an org file — the `[[file:path]]` links exist in the
    /// org text, but the actual binary files may not yet be on disk. Reads bytes
    /// from the `ImageDataProvider` and writes to `{root_dir}/{block.content}`.
    /// Skips blocks whose files already exist.
    async fn materialize_images(&self, doc_id: &EntityUri) -> Result<()> {
        let Some(ref provider) = self.image_data else {
            return Ok(());
        };
        let blocks = self.block_reader.get_blocks(doc_id).await?;

        for block in blocks.iter().filter(|b| b.is_image_block()) {
            let image_path = self.resolve_image_path(&block.content)?;
            if image_path.exists() {
                continue;
            }

            let data = provider.read_image_data(&block.id).await.with_context(|| {
                format!(
                    "Failed to read image data for block {} (path: {})",
                    block.id, block.content
                )
            })?;

            let Some(data) = data else {
                debug!(
                    "[OrgSyncController] No image data stored for block {} — \
                     file {} will be missing on disk",
                    block.id, block.content
                );
                continue;
            };

            if let Some(parent) = image_path.parent() {
                tokio::fs::create_dir_all(parent).await?;
            }
            tokio::fs::write(&image_path, &data)
                .await
                .with_context(|| {
                    format!(
                        "Failed to write image file {} for block {}",
                        image_path.display(),
                        block.id
                    )
                })?;
            info!(
                "[OrgSyncController] Materialized image {} ({} bytes)",
                image_path.display(),
                data.len()
            );
        }
        Ok(())
    }

    /// Read image files from disk and store them via `ImageDataProvider`.
    ///
    /// Called after parsing an org file that contains `[[file:path]]` image links.
    /// The blocks have been created in the store, but the binary data needs to be
    /// ingested so it's available for cross-peer sync and Loro storage.
    async fn ingest_images(&self, doc_id: &EntityUri) -> Result<()> {
        let Some(ref provider) = self.image_data else {
            return Ok(());
        };
        let blocks = self.block_reader.get_blocks(doc_id).await?;

        for block in blocks.iter().filter(|b| b.is_image_block()) {
            let image_path = match self.resolve_image_path(&block.content) {
                Ok(p) => p,
                Err(e) => {
                    debug!(
                        "[OrgSyncController] Skipping image ingestion for block {}: {}",
                        block.id, e
                    );
                    continue;
                }
            };
            if !image_path.exists() {
                continue;
            }

            let data = tokio::fs::read(&image_path).await.with_context(|| {
                format!(
                    "Failed to read image file {} for block {}",
                    image_path.display(),
                    block.id
                )
            })?;
            provider
                .write_image_data(&block.id, data)
                .await
                .with_context(|| {
                    format!(
                        "Failed to store image data for block {} (path: {})",
                        block.id, block.content
                    )
                })?;
            info!(
                "[OrgSyncController] Ingested image {} for block {}",
                image_path.display(),
                block.id
            );
        }
        Ok(())
    }

    /// Resolve a relative image path to an absolute path under root_dir.
    /// Returns Err if the resolved path escapes the root directory (path traversal).
    fn resolve_image_path(&self, relative_path: &str) -> Result<PathBuf> {
        let joined = self.root_dir.join(relative_path);
        let canonical_root =
            std::fs::canonicalize(&self.root_dir).unwrap_or_else(|_| self.root_dir.clone());
        // For paths that don't exist yet, canonicalize the parent and append the filename
        let resolved = if joined.exists() {
            std::fs::canonicalize(&joined)?
        } else if let Some(parent) = joined.parent() {
            let canonical_parent = if parent.exists() {
                std::fs::canonicalize(parent)?
            } else {
                parent.to_path_buf()
            };
            canonical_parent.join(joined.file_name().unwrap_or_default())
        } else {
            joined.clone()
        };
        assert!(
            resolved.starts_with(&canonical_root) || joined.starts_with(&self.root_dir),
            "Image path traversal blocked: {} resolves to {} which is outside {}",
            relative_path,
            resolved.display(),
            self.root_dir.display()
        );
        Ok(joined)
    }

    /// Run the post-org-write hook (fire-and-forget).
    fn run_post_write_hook(&self, path: &Path) {
        let Some(ref cmd) = self.post_org_write_hook else {
            return;
        };
        let cmd = cmd.clone();
        let root_dir = self.root_dir.clone();
        let file_path = path.to_path_buf();
        tokio::spawn(async move {
            let result = tokio::process::Command::new("sh")
                .arg("-l")
                .arg("-c")
                .arg(&cmd)
                .current_dir(&root_dir)
                .env("HOLON_FILE", &file_path)
                .output()
                .await;
            match result {
                Ok(output) if output.status.success() => {
                    info!(
                        "[OrgSyncController] post_org_write hook succeeded for {}",
                        file_path.display()
                    );
                }
                Ok(output) => {
                    tracing::warn!(
                        "[OrgSyncController] post_org_write hook failed (exit={}) for {}: {}",
                        output.status,
                        file_path.display(),
                        String::from_utf8_lossy(&output.stderr),
                    );
                }
                Err(e) => {
                    tracing::warn!(
                        "[OrgSyncController] post_org_write hook spawn failed for {}: {}",
                        file_path.display(),
                        e,
                    );
                }
            }
        });
    }

    /// Resolve a doc_id to a filesystem path via DocumentManager.
    async fn doc_id_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        // Try alias registrar first (fastest path)
        if let Some(ref registrar) = self.alias_registrar {
            if let Some(path) = registrar.resolve_alias_to_path(doc_id).await {
                return Some(path);
            }
        }

        // Walk the Document hierarchy to compute the path
        match self.doc_manager.name_chain(doc_id).await {
            Ok(chain) if !chain.is_empty() => {
                let path = self.root_dir.join(chain.join("/")).with_extension("org");
                Some(path)
            }
            Ok(_) => None,
            Err(_) => None,
        }
    }
}

/// Convert a relative path (e.g. "projects/todo.org") to a name chain (["projects", "todo"]).
fn path_to_name_chain(rel_path: &Path) -> Vec<String> {
    let doc_path = rel_path.with_extension("");
    doc_path
        .components()
        .filter_map(|c| c.as_os_str().to_str().map(|s| s.to_string()))
        .collect()
}

/// Check if two blocks differ in ways that require an update.
/// Phase 2: when an UPDATE op's edge sets (`tags`, `requires`) match the
/// old block's, strip those keys from `params` so the provider doesn't
/// emit a wipe-and-rebuild on the `block_tags` / `block_requires` junction.
///
/// Compares as `HashSet<&str>` because junction reads have undefined row
/// order; vector compare would flag false diffs.
fn strip_unchanged_edge_fields(
    params: &mut HashMap<String, Value>,
    old_block: &Block,
    new_block: &Block,
) {
    if set_eq(&old_block.tags, &new_block.tags) {
        params.remove("tags");
    }
    if set_eq(&old_block.requires, &new_block.requires) {
        params.remove("requires");
    }
}

fn set_eq(a: &[String], b: &[String]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let sa: HashSet<&str> = a.iter().map(String::as_str).collect();
    let sb: HashSet<&str> = b.iter().map(String::as_str).collect();
    sa == sb
}

fn blocks_differ(a: &Block, b: &Block) -> bool {
    a.content != b.content
        || a.parent_id != b.parent_id
        || a.content_type != b.content_type
        || a.source_language != b.source_language
        || a.source_name != b.source_name
        || a.task_state() != b.task_state()
        || a.priority() != b.priority()
        || a.tags() != b.tags()
        || a.scheduled() != b.scheduled()
        || a.deadline() != b.deadline()
        || a.drawer_properties() != b.drawer_properties()
        || a.sequence() != b.sequence()
        // sort_key must be checked too: the parser assigns per-parent
        // fractional indices via `gen_n_keys(N)` where `N` is the parsed
        // sibling count. When a file is re-parsed with a different sibling
        // count (e.g. bulk-add), every sibling gets a fresh fractional
        // index drawn from a new keyspace. Without re-issuing updates for
        // existing blocks they retain stale keys from the previous parse,
        // and `BlockOperations::get_prev_sibling` (filter `s.sort_key < b.sort_key`)
        // fails because the two keyspaces are not order-comparable.
        || a.sort_key != b.sort_key
}

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
use holon::sync::CanonicalPath;
use holon_api::block::Block;
use holon_api::{EntityName, EntityUri, Value};
use holon_core::file_format::FileFormatAdapter;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::block_params::build_block_params;
use crate::file_format::OrgFormatAdapter;
use crate::models::{OrgBlockExt, OrgDocumentExt};
use crate::parser::generate_file_id;
use crate::traits::{BlockReader, DocumentManager, ImageDataProvider};

pub struct OrgSyncController {
    /// What we last wrote to (or confirmed on) disk, per file.
    /// Uses CanonicalPath to resolve macOS /var → /private/var symlinks,
    /// so scan_org_files and file watcher events match the same key.
    last_projection: HashMap<CanonicalPath, String>,

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
    ) -> Self {
        Self::with_format(
            block_reader,
            command_bus,
            doc_manager,
            root_dir,
            Arc::new(OrgFormatAdapter::new()),
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
    ) -> Self {
        // Canonicalize root_dir so strip_prefix works with canonical file paths
        // (macOS: /var → /private/var symlink resolution).
        let root_dir = CanonicalPath::new(&root_dir).into_path_buf();
        Self {
            last_projection: HashMap::new(),
            block_reader,
            command_bus,
            doc_manager,
            root_dir,
            alias_registrar: None,
            post_org_write_hook: None,
            image_data: None,
            format,
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
        let documents = self.block_reader.iter_documents_with_blocks().await?;

        for (doc_id, blocks) in documents {
            let file_path = match self.resolve_doc_to_path(&doc_id).await {
                Some(p) => p,
                None => {
                    debug!(
                        "[OrgSyncController] Skipping doc_id={} — cannot resolve to file path",
                        doc_id
                    );
                    continue;
                }
            };
            let rendered = match self.doc_manager.get_by_id(&doc_id).await {
                Ok(Some(doc)) => self
                    .format
                    .render_document(&doc, &blocks, &file_path, &doc_id),
                _ => self.format.render_blocks(&blocks, &file_path, &doc_id),
            };
            self.last_projection
                .insert(CanonicalPath::new(&file_path), rendered);
        }

        info!(
            "[OrgSyncController] Initialized last_projection for {} files",
            self.last_projection.len()
        );
        Ok(())
    }

    /// Resolve a document ID to a file path under root_dir.
    async fn resolve_doc_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        // Try alias registrar first (Loro path stores UUID→path mapping)
        if let Some(ref registrar) = self.alias_registrar {
            if let Some(path) = registrar.resolve_alias_to_path(&doc_id).await {
                return Some(path);
            }
        }
        // Fall back to name_chain → file path
        match self.doc_manager.name_chain(doc_id).await {
            Ok(chain) if !chain.is_empty() => {
                let file_name = format!("{}.org", chain.join("/"));
                Some(self.root_dir.join(file_name))
            }
            _ => None,
        }
    }

    /// Handle a file change event from the FileWatcher.
    ///
    /// Echo suppression: if disk content matches last_projection, skip.
    /// Otherwise, diff against last_projection to compute create/update/delete ops.
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

        // Skip 0-byte files that are not yet tracked. Empty files have no
        // blocks to sync; registering them only causes re_render_all_tracked
        // to fail with "No document found" on every subsequent block event.
        if disk_content.is_empty() && !self.last_projection.contains_key(&canonical) {
            debug!(
                "[OrgSyncController] Skipping empty file (not tracked): {}",
                path.display()
            );
            return Ok(());
        }

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

        // Get or create the document entity for this file
        let segments = path_to_name_chain(rel_path);
        let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
        let document = self
            .doc_manager
            .get_or_create_by_name_chain(&segment_refs)
            .await?;
        let document_uri = document.id.clone();

        // Register UUID → file path alias (if Loro is available)
        if let Some(ref registrar) = self.alias_registrar {
            registrar.register_alias(&document_uri, path).await;
        }

        let canonical_path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let canonical_root =
            std::fs::canonicalize(&self.root_dir).unwrap_or_else(|_| self.root_dir.clone());
        let file_id = generate_file_id(&canonical_path, &canonical_root);

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
        for block in &new_blocks_vec {
            if !old_blocks.contains_key(&block.id) {
                let parent_id = if block.parent_id == file_id {
                    &document_uri
                } else {
                    &block.parent_id
                };
                let params = build_block_params(block, parent_id, &document_uri);
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

        // Updates
        for (id, new_block) in &new_blocks {
            if let Some(old_block) = old_blocks.get(id) {
                if blocks_differ(old_block, new_block) {
                    let parent_id = if new_block.parent_id == file_id {
                        &document_uri
                    } else {
                        &new_block.parent_id
                    };
                    let params = build_block_params(new_block, parent_id, &document_uri);
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
                operations.push(("delete".to_string(), params));
            }
        }

        // Execute all operations as a single batch (one transaction + one event batch)
        let expected_block_count = new_blocks.len();
        if !operations.is_empty() {
            self.command_bus
                .execute_batch(&EntityName::new("block"), operations)
                .await
                .map_err(|e| {
                    anyhow::anyhow!(
                        "Batch block operations failed for {}: {}",
                        path.display(),
                        e
                    )
                })?;

            // Wait for the CDC-driven cache to reflect the committed changes.
            // Without this, render_file_by_doc_id reads stale data and overwrites
            // the file with fewer blocks than what we just ingested.
            let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_millis(2000);
            loop {
                let cached_blocks = self.block_reader.get_blocks(&document_uri).await?;
                if cached_blocks.len() >= expected_block_count {
                    break;
                }
                if tokio::time::Instant::now() >= deadline {
                    anyhow::bail!(
                        "[on_file_changed] CDC cache did not catch up within 2s for {} \
                         (expected {} blocks, cache has {})",
                        path.display(),
                        expected_block_count,
                        cached_blocks.len()
                    );
                }
                tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;
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
        if !has_structural_changes {
            self.last_projection
                .insert(canonical.clone(), disk_content.to_string());
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

        // Update last_projection
        self.last_projection.insert(canonical.clone(), rendered);

        Ok(())
    }

    /// Handle a block change notification (from EventBus or Loro).
    ///
    /// Re-renders the affected file and writes if content changed.
    /// Returns `true` if a matching document file was found and re-rendered,
    /// `false` if the doc_id didn't map to any known file.
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
    pub async fn poll_external_changes(&mut self) -> Result<usize> {
        let keys: Vec<(CanonicalPath, PathBuf)> = self
            .last_projection
            .iter()
            .map(|(k, _)| (k.clone(), (**k).to_path_buf()))
            .collect();

        let mut ingested = 0;
        for (canonical, path) in keys {
            let disk_content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    return Err(e).with_context(|| {
                        format!("[poll_external_changes] Cannot read {}", path.display())
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
                    "[OrgSyncController] poll_external_changes: ingesting {} (disk != last_projection)",
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
                    // Log once at warn and continue — don't fail the batch.
                    warn!(
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

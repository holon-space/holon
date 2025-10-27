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

use anyhow::Result;
use holon::core::datasource::OperationProvider;
use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_api::EntityUri;
use holon_api::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tracing::{debug, info, warn};

use crate::models::OrgBlockExt;
use crate::org_renderer::OrgRenderer;
use crate::parser::{generate_file_id, parse_org_file};
use crate::traits::{BlockReader, DocumentManager};

pub struct OrgSyncController {
    /// What we last wrote to (or confirmed on) disk, per file.
    last_projection: HashMap<PathBuf, String>,

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
        Self {
            last_projection: HashMap::new(),
            block_reader,
            command_bus,
            doc_manager,
            root_dir,
            alias_registrar: None,
            post_org_write_hook: None,
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

    /// Initialize last_projection from the block reader's current state.
    ///
    /// Must be called at startup BEFORE scanning files, so that we have a
    /// diff base for detecting external edits.
    pub async fn initialize(&mut self) {
        let documents = self.block_reader.iter_documents_with_blocks().await;

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
                Ok(Some(doc)) => OrgRenderer::render_document(&doc, &blocks, &file_path, &doc_id),
                _ => OrgRenderer::render_blocks(&blocks, &file_path, &doc_id),
            };
            self.last_projection.insert(file_path, rendered);
        }

        info!(
            "[OrgSyncController] Initialized last_projection for {} files",
            self.last_projection.len()
        );
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
        let disk_content = match tokio::fs::read_to_string(path).await {
            Ok(c) => c,
            Err(e) => {
                debug!(
                    "[OrgSyncController] Cannot read {}: {} (deleted?)",
                    path.display(),
                    e
                );
                return Ok(());
            }
        };

        let last = self
            .last_projection
            .get(path)
            .map(|s| s.as_str())
            .unwrap_or("");

        // Echo suppression: skip if we have a prior projection and content matches.
        // An absent entry means "first time seeing this file" — always process it
        // to create the document entity (needed for block→file sync).
        if self.last_projection.contains_key(path) && disk_content == last {
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

        let file_id = generate_file_id(path, &self.root_dir);

        // Parse old state (from last_projection) and new state (from disk)
        let old_blocks = if last.is_empty() {
            HashMap::new()
        } else {
            match parse_org_file(path, last, &EntityUri::doc_root(), 0, &self.root_dir) {
                Ok(result) => result
                    .blocks
                    .into_iter()
                    .map(|b| (b.id.clone(), b))
                    .collect(),
                Err(_) => HashMap::new(),
            }
        };

        let new_parse = parse_org_file(
            path,
            &disk_content,
            &EntityUri::doc_root(),
            0,
            &self.root_dir,
        )?;
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
        assert!(
            conflicts.is_empty(),
            "[OrgSyncController] Duplicate block IDs across files! \
             File {} contains block IDs that already exist in other documents: {:?}. \
             Org :ID: properties must be globally unique.",
            path.display(),
            conflicts,
        );

        // Collect all block operations into a batch
        let mut operations: Vec<(String, HashMap<String, Value>)> = Vec::new();

        // Creates (in document order so parents before children)
        for block in &new_blocks_vec {
            if !old_blocks.contains_key(&block.id) {
                let parent_id = if block.parent_id == file_id {
                    &document_uri
                } else {
                    &block.parent_id
                };
                let params = build_block_params(block, parent_id, &document_uri);
                operations.push(("create".to_string(), params));
            }
        }

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
                let mut params = HashMap::new();
                params.insert("id".to_string(), Value::String(id.to_string()));
                operations.push(("delete".to_string(), params));
            }
        }

        // Execute all operations as a single batch (one transaction + one event batch)
        let expected_block_count = new_blocks.len();
        if !operations.is_empty() {
            self.command_bus
                .execute_batch("block", operations)
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

        // Re-project from block reader (cache is now up-to-date)
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
            tokio::fs::write(path, &rendered).await?;
            self.run_post_write_hook(path);
            info!(
                "[OrgSyncController] Wrote merged content to {}",
                path.display()
            );
        }

        // Update last_projection
        self.last_projection.insert(path.to_path_buf(), rendered);

        Ok(())
    }

    /// Handle a block change notification (from EventBus or Loro).
    ///
    /// Re-renders the affected file and writes if content changed.
    pub async fn on_block_changed(&mut self, doc_id: &EntityUri) -> Result<()> {
        let path = match self.doc_id_to_path(doc_id).await {
            Some(p) => p,
            None => return Ok(()),
        };

        // If disk content differs from last_projection, there's a pending external
        // change that the file watcher hasn't delivered yet. Ingest it first so
        // the re-render below includes both the block event and the external edit.
        let disk_content = tokio::fs::read_to_string(&path).await.unwrap_or_default();
        let last = self
            .last_projection
            .get(&path)
            .map(|s| s.as_str())
            .unwrap_or("");
        if disk_content != last {
            info!(
                "[OrgSyncController] Processing pending external change for {} before re-render",
                path.display()
            );
            self.on_file_changed(&path).await?;
        }

        let rendered = self.render_file_by_doc_id(doc_id, &path).await?;

        let current_last = self
            .last_projection
            .get(&path)
            .map(|s| s.as_str())
            .unwrap_or("");

        if rendered == current_last {
            return Ok(());
        }

        tokio::fs::write(&path, &rendered).await?;
        self.run_post_write_hook(&path);
        self.last_projection.insert(path.clone(), rendered);

        info!(
            "[OrgSyncController] Wrote block changes to {}",
            path.display()
        );

        Ok(())
    }

    /// Re-render all tracked files (used for events where the doc_id is unknown,
    /// e.g. block.deleted, block.fields_changed).
    pub async fn re_render_all_tracked(&mut self) -> Result<()> {
        let paths: Vec<PathBuf> = self.last_projection.keys().cloned().collect();

        for path in paths {
            // If disk content differs from last_projection, ingest the pending external
            // change first so the re-render includes both the block event and external edit.
            let disk_content = match tokio::fs::read_to_string(&path).await {
                Ok(c) => c,
                Err(e) => {
                    warn!(
                        "[re_render_all_tracked] Cannot read {}: {}",
                        path.display(),
                        e
                    );
                    continue;
                }
            };
            let last = self
                .last_projection
                .get(&path)
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
            let rel_path = match path.strip_prefix(&self.root_dir) {
                Ok(r) => r,
                Err(e) => {
                    warn!(
                        "[re_render_all_tracked] {} not under root_dir {}: {}",
                        path.display(),
                        self.root_dir.display(),
                        e
                    );
                    continue;
                }
            };
            let segments = path_to_name_chain(rel_path);
            let segment_refs: Vec<&str> = segments.iter().map(|s| s.as_str()).collect();
            let doc = match self.doc_manager.find_by_name_chain(&segment_refs).await {
                Ok(Some(d)) => d,
                Ok(None) => {
                    warn!(
                        "[re_render_all_tracked] No document found for path {} (segments: {:?})",
                        path.display(),
                        segment_refs
                    );
                    continue;
                }
                Err(e) => {
                    warn!(
                        "[re_render_all_tracked] Doc lookup failed for {}: {}",
                        path.display(),
                        e
                    );
                    continue;
                }
            };

            let rendered = self.render_file_by_doc_id(&doc.id, &path).await?;

            let current_last = self
                .last_projection
                .get(&path)
                .map(|s| s.as_str())
                .unwrap_or("");

            if rendered == current_last {
                continue;
            }

            tokio::fs::write(&path, &rendered).await?;
            self.run_post_write_hook(&path);
            self.last_projection.insert(path.clone(), rendered);

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
            Some(doc) => OrgRenderer::render_document(&doc, &blocks, path, doc_id),
            None => OrgRenderer::render_blocks(&blocks, path, doc_id),
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

/// Build command parameters for a block create/update operation.
pub fn build_block_params(
    block: &Block,
    parent_id: &EntityUri,
    document_uri: &EntityUri,
) -> HashMap<String, Value> {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(block.id.to_string()));
    params.insert(
        "parent_id".to_string(),
        Value::String(parent_id.to_string()),
    );
    params.insert(
        "document_id".to_string(),
        Value::String(document_uri.to_string()),
    );
    params.insert("content".to_string(), Value::String(block.content.clone()));
    params.insert(
        "content_type".to_string(),
        Value::String(block.content_type.to_string()),
    );

    // Timestamps must be provided explicitly as integers (millis).
    // The blocks table DDL has `DEFAULT (datetime('now'))` which produces TEXT,
    // but Block::from_entity expects i64. Always provide integer timestamps
    // to avoid this mismatch.
    let now = chrono::Utc::now().timestamp_millis();
    let created = if block.created_at > 0 {
        block.created_at
    } else {
        now
    };
    params.insert("created_at".to_string(), Value::Integer(created));
    params.insert("updated_at".to_string(), Value::Integer(now));

    if block.content_type == ContentType::Source {
        if let Some(ref lang) = block.source_language {
            params.insert(
                "source_language".to_string(),
                Value::String(lang.to_string()),
            );
        }
        if let Some(ref name) = block.source_name {
            params.insert("source_name".to_string(), Value::String(name.clone()));
        }
        let header_args = block.get_source_header_args();
        if !header_args.is_empty() {
            if let Ok(json) = serde_json::to_string(&header_args) {
                params.insert("source_header_args".to_string(), Value::String(json));
            }
        }
    }

    if let Some(task_state) = block.task_state() {
        params.insert(
            "task_state".to_string(),
            Value::String(task_state.to_string()),
        );
    }
    if let Some(priority) = block.priority() {
        params.insert(
            "priority".to_string(),
            Value::Integer(priority.to_int() as i64),
        );
    }
    let tags = block.tags();
    if !tags.is_empty() {
        params.insert("tags".to_string(), Value::String(tags.to_csv()));
    }
    if let Some(scheduled) = block.scheduled() {
        params.insert(
            "scheduled".to_string(),
            Value::String(scheduled.to_string()),
        );
    }
    if let Some(deadline) = block.deadline() {
        params.insert("deadline".to_string(), Value::String(deadline.to_string()));
    }

    params.insert("sequence".to_string(), Value::Integer(block.sequence()));

    // Include org drawer properties (flat in block.properties)
    let id = block
        .get_block_id()
        .unwrap_or_else(|| block.id.id().to_string());
    params.insert("ID".to_string(), Value::String(id));

    for (k, v) in block.drawer_properties() {
        params.insert(k, Value::String(v));
    }

    params
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
}

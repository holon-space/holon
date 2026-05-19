//! Dependency Injection module for OrgMode integration
//!
//! This module provides DI registration for OrgMode-specific services using fluxdi.
//! OrgMode is now independent of Loro — it will use LoroBlockOperations if available in DI,
//! otherwise falls back to SqlOperationProvider for direct database writes.
//!
//! # Usage
//!
//! ```rust,ignore
//! use holon_orgmode::di::OrgModeInjectorExt;
//! use std::path::PathBuf;
//!
//! services.add_orgmode(PathBuf::from("/path/to/org/files"))?;
//! ```

use std::path::{Path, PathBuf};
use std::sync::Arc;

use fluxdi::{Injector, Module, Provider, Shared};

use holon_filesystem::directory::Directory;
use holon_filesystem::File;

use crate::file_watcher::OrgFileWatcher;
use crate::org_renderer::OrgRenderer;
use crate::org_sync_controller::OrgSyncController;
use crate::orgmode_event_adapter::OrgModeEventAdapter;
use crate::traits::{BlockReader, DocumentManager};
use crate::OrgModeSyncProvider;
use holon::core::datasource::{OperationProvider, SyncTokenStore, SyncableProvider};
use holon::core::operation_wrapper::OperationWrapper;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::schema_module::SchemaModule;
use holon::storage::schema_modules::BlockSchemaModule;
use holon::storage::{BLOCK_READ_TABLE, BLOCK_WRITE_TABLE};
use holon::sync::event_bus::EventOrigin;
use holon::sync::event_bus::{EventBus, PublishErrorTracker};
use holon::sync::{LoroBlockOperations, LoroDocumentStore, TursoEventBus};
use holon::type_registry::TypeRegistry;
use holon_api::block::{blocks_by_document, Block};
use holon_api::{EntityName, EntityUri};
use holon_core::block_ordering::BlockOrdering;

/// Signal that indicates the FileWatcher is ready to receive file change events.
///
/// Tests can wait on this signal to ensure the file watcher is established
/// before making external file modifications.
#[derive(Clone)]
pub struct FileWatcherReadySignal {
    receiver: tokio::sync::watch::Receiver<Option<Result<(), String>>>,
}

impl FileWatcherReadySignal {
    /// Create a new ready signal (sender/receiver pair)
    pub fn new() -> (FileWatcherReadySender, Self) {
        let (tx, rx) = tokio::sync::watch::channel(None);
        (FileWatcherReadySender { sender: tx }, Self { receiver: rx })
    }

    /// Check if startup has completed (either success or failure).
    pub fn is_completed(&self) -> bool {
        self.receiver.borrow().is_some()
    }

    /// Wait until the file watcher signals readiness.
    ///
    /// Returns `Ok(())` on success, `Err` if the OrgSyncController startup failed.
    /// Errors are propagated — never swallowed.
    #[tracing::instrument(skip(self), name = "FileWatcherReadySignal.wait_ready")]
    pub async fn wait_ready(&self) -> anyhow::Result<()> {
        let mut receiver = self.receiver.clone();
        // Wait until the value is Some(_)
        let result = receiver.wait_for(|v| v.is_some()).await.map_err(|_| {
            anyhow::anyhow!("FileWatcherReadySignal sender dropped without signaling")
        })?;
        match result.as_ref().unwrap() {
            Ok(()) => Ok(()),
            Err(msg) => Err(anyhow::anyhow!("OrgSyncController startup failed: {}", msg)),
        }
    }
}

/// Sender half of the FileWatcher ready signal
pub struct FileWatcherReadySender {
    sender: tokio::sync::watch::Sender<Option<Result<(), String>>>,
}

impl FileWatcherReadySender {
    /// Signal successful readiness.
    pub fn signal_ready(self) {
        let _ = self.sender.send(Some(Ok(())));
    }

    /// Signal that startup failed. The error message propagates to the waiter.
    pub fn signal_error(self, error: String) {
        let _ = self.sender.send(Some(Err(error)));
    }
}

/// Event-driven idle signal for the OrgSyncController loop.
///
/// The controller's background task calls [`mark_progress`] after each
/// iteration where it actually processed an event (file change or block
/// change). Tests use [`wait_quiescent`] to wait until the loop has had no
/// activity for a short window — proving that all org-file writes triggered
/// by recent SQL mutations have already landed on disk.
///
/// This replaces filesystem mtime polling on the hot path (~30 ms per call)
/// with an event signal that completes in ~1 ms when the loop is genuinely
/// idle. Callers that don't have access to the signal (or want extra safety)
/// fall back to mtime polling.
///
/// [`mark_progress`]: OrgSyncIdleSignal::mark_progress
/// [`wait_quiescent`]: OrgSyncIdleSignal::wait_quiescent
#[derive(Debug)]
pub struct OrgSyncIdleSignal {
    /// Monotonic count of completed loop iterations. Bumped after every
    /// processed event (file or block change).
    tick: std::sync::atomic::AtomicU64,
    /// Wakes any task waiting in [`wait_quiescent`] whenever the tick advances.
    notify: tokio::sync::Notify,
}

impl OrgSyncIdleSignal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tick: std::sync::atomic::AtomicU64::new(0),
            notify: tokio::sync::Notify::new(),
        })
    }

    /// Current tick value. Increases monotonically.
    pub fn current_tick(&self) -> u64 {
        self.tick.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Called by the controller loop after each processed event.
    pub fn mark_progress(&self) {
        self.tick.fetch_add(1, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Wait until the controller loop has been idle (no [`mark_progress`]
    /// call) for `quiescence`, or `timeout` elapses. Returns `true` if
    /// quiescence was reached, `false` on timeout.
    ///
    /// Cost when already idle: one `tokio::time::timeout` of `quiescence`.
    /// Cost when busy: as long as it takes for the loop to drain, capped by
    /// `timeout`.
    ///
    /// [`mark_progress`]: Self::mark_progress
    pub async fn wait_quiescent(
        &self,
        quiescence: std::time::Duration,
        timeout: std::time::Duration,
    ) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let snapshot = self.current_tick();
            // Subscribe BEFORE re-reading the tick to avoid missing a wake.
            let notified = self.notify.notified();
            if self.current_tick() != snapshot {
                // Activity already happened; loop again.
                if tokio::time::Instant::now() >= deadline {
                    return false;
                }
                continue;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            let wait = quiescence.min(remaining);
            match tokio::time::timeout(wait, notified).await {
                Err(_) => {
                    // No notification within `quiescence` — the loop is idle.
                    if self.current_tick() == snapshot {
                        return true;
                    }
                    // A wake landed between the timeout firing and the re-check;
                    // treat it as activity and loop.
                }
                Ok(()) => {
                    // Got woken — keep waiting unless we ran out of time.
                    if tokio::time::Instant::now() >= deadline {
                        return false;
                    }
                }
            }
        }
    }
}

/// Scan a directory recursively for .org files.
///
/// Delegates to `file_watcher::scan_directory` — the single source of truth
/// for directory walking (respects .gitignore, skips .git/.jj).
fn scan_org_files(dir: &std::path::Path) -> Vec<PathBuf> {
    crate::file_watcher::scan_directory(dir).files
}

// =============================================================================
// Trait implementations for decoupling org-mode from Loro/Turso
// =============================================================================

/// BlockReader backed by `QueryableCache<Block>`.
///
/// Reads bypass `cache.get_all()` because edge-typed fields like `tags`
/// live in junction tables (`block_tags`, `block_requires`) — a plain
/// `SELECT * FROM block` returns rows with `tags = []` and consumers like
/// the org renderer silently drop the headline tag. Instead, every read
/// goes through `load_all_blocks_with_hydration` which adds correlated
/// subqueries against the junctions so the materialized `Block` values
/// arrive with edge fields populated.
///
/// Junction-aware hydration in this read path also means downstream code
/// (renderer, MCP, link-provider, …) never has to know about junction
/// tables — that storage split stays Turso's concern.
pub struct CacheBlockReader {
    cache: Arc<QueryableCache<Block>>,
    /// Phase 4: drives `wait_for_cache_caught_up`. `None` for backends
    /// without an ack pipeline (only happens in tests that don't wire
    /// up `TursoEventBus`).
    event_bus: Option<Arc<TursoEventBus>>,
}

impl CacheBlockReader {
    pub fn new(cache: Arc<QueryableCache<Block>>) -> Self {
        Self {
            cache,
            event_bus: None,
        }
    }

    /// Wire the TursoEventBus so `wait_for_cache_caught_up` can replace
    /// the 10 ms full-scan poll with a push-based wait on the cache
    /// consumer's ack watermark.
    pub fn with_event_bus(mut self, event_bus: Arc<TursoEventBus>) -> Self {
        self.event_bus = Some(event_bus);
        self
    }

    /// Load every block from `block_raw` with edge-typed fields hydrated
    /// via correlated subquery against `block_tags`. Add more junction
    /// joins here as new edge fields appear.
    ///
    /// Returns `Vec<Block>` with `block.tags` populated. Skipping the cache's
    /// generic `SELECT *` keeps the contract uniform: every consumer of
    /// `BlockReader` sees the same hydrated shape.
    ///
    /// Why `block_raw` and not the `block` matview: matview rows propagate
    /// via CDC, which lags the underlying SQL write by an unbounded window.
    /// `on_block_changed` runs on an `events` CDC delivery whose source
    /// transaction has already committed to `block_raw`, but the `block`
    /// matview may not yet reflect it. Reading the matview here renders a
    /// stale snapshot and writes a stale org file. Same race class as
    /// inv-viewmodel-root-matches-render-expr (`block_with_query_source.sql` → `block_raw`); see
    /// devlog/2026-05-05-110315.md.
    async fn load_all_blocks_with_hydration(&self) -> anyhow::Result<Vec<Block>> {
        let sql = format!(
            "SELECT b.id, b.parent_id, b.depth, b.sort_key, b.content, \
             b.content_type, b.source_language, b.source_name, \
             b.properties, b.marks, b.collapsed, b.completed, \
             b.block_type, b.created_at, b.updated_at, \
             COALESCE((SELECT json_group_array(tag) FROM block_tags WHERE block_id = b.id), '[]') AS tags \
             FROM {BLOCK_WRITE_TABLE} b"
        );

        let rows = self
            .cache
            .db_handle()
            .query(&sql, std::collections::HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("[CacheBlockReader] hydrating SELECT failed: {e}"))?;

        // NOTE: must go through `Block: TryFrom<HashMap<String, Value>>`,
        // NOT `Block: TryFromEntity`. `tags` is `#[serde(skip, default)]`,
        // and the `Entity` derive treats that as "skip serialization" — so
        // the derived `from_entity` would silently leave `tags` empty even
        // when the row carries a populated `tags` JSON array. The hand-rolled
        // `TryFrom` impl in `holon-api/src/block.rs` parses the column.
        rows.into_iter()
            .map(|row| {
                Block::try_from(row).map_err(|e| {
                    anyhow::anyhow!("[CacheBlockReader] Block::try_from row failed: {e}")
                })
            })
            .collect()
    }
}

#[async_trait::async_trait]
impl BlockReader for CacheBlockReader {
    async fn get_blocks(&self, doc_id: &EntityUri) -> anyhow::Result<Vec<Block>> {
        // Phase 5: push the doc-scoped BFS into SQL via a recursive CTE on
        // `block_raw`. Replaces `load_all_blocks_with_hydration` + in-Rust
        // BFS — that combination fired ~26×/sec on the
        // `on_block_changed → render_file_by_doc_id` path (one full table
        // scan per Loro→SQL block apply). The CTE walks down from
        // `doc_id`'s direct children, excluding any block tagged `Page`
        // (sub-document boundary; the Rust BFS did the same by skipping
        // `block.is_page()`). Depth bound 100 matches existing
        // `find_document_uri` shape.
        let sql = format!(
            "WITH RECURSIVE descendants(id, depth_acc) AS ( \
                SELECT b.id, 0 \
                FROM {table} b \
                LEFT JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page' \
                WHERE b.parent_id = $doc_id \
                  AND bt.block_id IS NULL \
                UNION ALL \
                SELECT b.id, d.depth_acc + 1 \
                FROM {table} b \
                JOIN descendants d ON b.parent_id = d.id \
                LEFT JOIN block_tags bt ON bt.block_id = b.id AND bt.tag = 'Page' \
                WHERE bt.block_id IS NULL AND d.depth_acc < 100 \
            ) \
            SELECT b.id, b.parent_id, b.depth, b.sort_key, b.content, \
                   b.content_type, b.source_language, b.source_name, \
                   b.properties, b.marks, b.collapsed, b.completed, \
                   b.block_type, b.created_at, b.updated_at, \
                   COALESCE((SELECT json_group_array(tag) FROM block_tags WHERE block_id = b.id), '[]') AS tags, \
                   COALESCE((SELECT json_group_array(required_id) FROM block_requires WHERE block_id = b.id), '[]') AS requires \
            FROM {table} b \
            JOIN descendants d ON d.id = b.id",
            table = BLOCK_WRITE_TABLE,
        );

        let mut params = std::collections::HashMap::new();
        params.insert(
            "doc_id".to_string(),
            holon_api::Value::String(doc_id.to_string()),
        );

        let rows = self
            .cache
            .db_handle()
            .query(&sql, params)
            .await
            .map_err(|e| anyhow::anyhow!("[CacheBlockReader::get_blocks] CTE query failed: {e}"))?;

        // Same Block::try_from path as load_all_blocks_with_hydration — the
        // derived TryFromEntity would silently leave `tags` empty because
        // of `#[serde(skip, default)]`. See block_two_deserializers memory.
        rows.into_iter()
            .map(|row| {
                Block::try_from(row).map_err(|e| {
                    anyhow::anyhow!(
                        "[CacheBlockReader::get_blocks] Block::try_from row failed: {e}"
                    )
                })
            })
            .collect()
    }

    async fn iter_documents_with_blocks(&self) -> anyhow::Result<Vec<(EntityUri, Vec<Block>)>> {
        let all_blocks = self.load_all_blocks_with_hydration().await?;
        Ok(blocks_by_document(&all_blocks))
    }

    /// Phase 1: load `(file_id, content_hash)` rows directly from the `file`
    /// table via raw SQL — bypasses the in-process file QueryableCache so we
    /// can read at controller startup, before CDC has replayed file events.
    async fn load_file_hashes(&self) -> anyhow::Result<Vec<(holon_api::EntityUri, String)>> {
        let rows = self
            .cache
            .db_handle()
            .query(
                "SELECT id, content_hash FROM file",
                std::collections::HashMap::new(),
            )
            .await
            .map_err(|e| anyhow::anyhow!("[CacheBlockReader] load_file_hashes failed: {e}"))?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let id = match row.get("id") {
                Some(holon_api::Value::String(s)) => s.clone(),
                _ => continue,
            };
            let hash = match row.get("content_hash") {
                Some(holon_api::Value::String(s)) => s.clone(),
                _ => String::new(),
            };
            if hash.is_empty() {
                // Skip rows the legacy writer left without a hash — they'd
                // false-match an empty stored value and incorrectly fast-path.
                continue;
            }
            let uri = holon_api::EntityUri::parse(&id).map_err(|e| {
                anyhow::anyhow!("[CacheBlockReader] file.id={id:?} not a valid EntityUri: {e}")
            })?;
            out.push((uri, hash));
        }
        Ok(out)
    }

    /// Phase 1 write-back: UPDATE `file.content_hash` for the given id.
    /// Bypasses the OperationProvider/event pipeline because:
    /// (a) the value is pure metadata (a hash), not domain data, so we
    /// don't want a CDC event for it; (b) `OrgSyncController` only reads
    /// `file.content_hash` at startup via `load_file_hashes` (raw SQL),
    /// not via the in-process cache — so cache staleness doesn't matter.
    /// Updates 0 rows silently when the file row hasn't been created yet
    /// by `OrgmodeSyncProvider` (first-ever boot case); the fast path
    /// engages on the next boot after the provider's sync creates it.
    /// Phase 4: replace the 10 ms `get_blocks().len()` poll. Uses the
    /// cache consumer's ack watermark exposed by `TursoEventBus`; falls
    /// back to instant `Ok(true)` when no event bus is wired (tests).
    async fn wait_for_cache_caught_up(
        &self,
        target_ts: i64,
        timeout_ms: u64,
    ) -> anyhow::Result<bool> {
        match &self.event_bus {
            Some(bus) => Ok(bus
                .wait_for_consumer_caught_up("cache", target_ts, timeout_ms)
                .await),
            None => Ok(true),
        }
    }

    async fn persist_file_hash(
        &self,
        file_id: &holon_api::EntityUri,
        hash: &str,
    ) -> anyhow::Result<()> {
        // Positional binds for db_handle().execute (it takes Vec<turso::Value>).
        let params = vec![
            turso::Value::Text(hash.to_string()),
            turso::Value::Text(file_id.to_string()),
        ];
        self.cache
            .db_handle()
            .execute("UPDATE file SET content_hash = ? WHERE id = ?", params)
            .await
            .map_err(|e| {
                anyhow::anyhow!("[CacheBlockReader] persist_file_hash UPDATE failed: {e}")
            })?;
        Ok(())
    }
}

/// DocumentManager backed by CDC-driven LiveData over page blocks.
///
/// All reads (`find_by_parent_and_name`, `get_by_id`) are in-memory lookups
/// against a `LiveData<Block>` that stays current via a Turso materialized
/// view CDC stream over blocks whose `tags` JSON list contains `"Page"`.
/// Writes go through `SqlOperationProvider` (SQL); the matview
/// CDC automatically propagates them into the LiveData.
pub struct LiveDocumentManager {
    live: Arc<holon::sync::LiveData<Block>>,
    command_bus: Arc<dyn OperationProvider>,
    /// Serializes find-then-create against itself so two concurrent
    /// `get_or_create_by_name_chain` calls for the same `(parent_id, title)`
    /// can't both miss the LiveData lookup and INSERT distinct UUIDs. The
    /// previous safeguard was `idx_block_document_unique` (UNIQUE on
    /// `(parent_id, name)`), which was dropped when `name` became a tag.
    create_lock: Arc<tokio::sync::Mutex<()>>,
}

impl LiveDocumentManager {
    /// Create a LiveDocumentManager backed by a materialized view over document blocks.
    pub async fn new(
        command_bus: Arc<dyn OperationProvider>,
        backend: Arc<tokio::sync::RwLock<holon::storage::turso::TursoBackend>>,
    ) -> anyhow::Result<Self> {
        let backend_guard = backend.read().await;
        let db_handle = backend_guard.handle();
        drop(backend_guard);

        let matview_mgr =
            holon::sync::MatviewManager::new(db_handle, Arc::new(tokio::sync::Mutex::new(())));

        // Match any block that has the "Page" tag in the block_tags junction table.
        let watch_sql = format!(
            "SELECT b.* FROM {BLOCK_READ_TABLE} b JOIN block_tags bt ON bt.block_id = b.id WHERE bt.tag = 'Page'"
        );
        let result = matview_mgr.watch(&watch_sql).await?;

        let live = holon::sync::LiveData::new(
            result.initial_rows,
            |row| {
                row.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s.to_string())
                    .ok_or_else(|| anyhow::anyhow!("document block row missing 'id'"))
            },
            |row| Block::try_from(row.clone()).map_err(|e| anyhow::anyhow!("{}", e)),
        );
        live.subscribe("document_blocks", result.stream);

        tracing::info!(
            "[LiveDocumentManager] Watching {} document blocks via matview",
            live.read().len()
        );

        Ok(Self {
            live,
            command_bus,
            create_lock: Arc::new(tokio::sync::Mutex::new(())),
        })
    }
}

#[async_trait::async_trait]
impl DocumentManager for LiveDocumentManager {
    async fn find_by_parent_and_name(
        &self,
        parent_id: &EntityUri,
        title: &str,
    ) -> anyhow::Result<Option<Block>> {
        let docs = self.live.read();
        Ok(docs
            .values()
            .find(|d| d.parent_id == *parent_id && d.is_page() && d.title() == title)
            .cloned())
    }

    async fn create(&self, doc: Block) -> anyhow::Result<Block> {
        use crate::block_params::build_block_params;

        // Serialize against concurrent creates so two callers asking for the
        // same `(parent_id, title)` page can't both observe LiveData empty
        // and both INSERT distinct UUIDs. Inside the lock, re-check LiveData;
        // if the page now exists (CDC may have caught up while we were
        // waiting), return the existing entry.
        let _guard = self.create_lock.lock().await;
        if let Some(existing) = self
            .find_by_parent_and_name(&doc.parent_id, &doc.title())
            .await?
        {
            tracing::debug!(
                "[LiveDocumentManager] Page {:?} already exists as {} (skipping create)",
                doc.title(),
                existing.id,
            );
            return Ok(existing);
        }

        // Route document creation events to the document's own ID.
        // _routing_doc_uri is only event routing metadata (not stored in DB) —
        // it tells OrgSyncController which file to re-render.
        let params = build_block_params(&doc, &doc.parent_id, &doc.id);
        // INSERT OR IGNORE: only triggers on PK collision now that the
        // partial unique index on `(parent_id, name)` is gone. The
        // `create_lock` above is what prevents same-title duplicates.
        // Tag the create event with `EventOrigin::Org` so the
        // `LoroSyncController` inbound gate routes it to `Apply` instead of
        // dropping it as a generic SQL-direct write. This page-creation flow
        // is triggered by `OrgSyncController::on_file_changed`; semantically
        // it's an Org-driven event.
        let result = self
            .command_bus
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "create",
                params,
                EventOrigin::Org,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;

        // If the response carries an existing id, the INSERT was ignored —
        // a page with the same (parent_id, title) already exists in the DB.
        // Return that existing page instead of the one we tried to insert.
        if let Some(holon_api::Value::String(existing_id)) = result.response {
            tracing::debug!(
                "[LiveDocumentManager] Page {:?} already exists as {} (attempted id={})",
                doc.title(),
                existing_id,
                doc.id,
            );
            let existing_uri = EntityUri::from_raw(&existing_id);
            if let Some(existing) = self.get_by_id(&existing_uri).await? {
                return Ok(existing);
            }
            // The document exists in SQL but not in LiveData.
            // Insert it so subsequent find_by_parent_and_name / get_by_id lookups succeed.
            let mut existing_doc = doc.clone();
            existing_doc.id = existing_uri;
            self.live
                .insert(existing_doc.id.as_str().to_string(), existing_doc.clone());
            return Ok(existing_doc);
        }

        self.live.insert(doc.id.as_str().to_string(), doc.clone());
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        let docs = self.live.read();
        Ok(docs.get(id.as_str()).cloned())
    }

    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        use crate::block_params::build_block_params;
        let params = build_block_params(doc, &doc.parent_id, &doc.id);
        // Tag as `EventOrigin::Org` mirroring sibling `create` above
        // (di.rs:551). Without this, the `LoroSyncController` inbound gate
        // drops the event as a generic SQL-direct write — and on doc rows
        // whose `properties` contain `todo_keywords` from a `#+TODO:`
        // directive, the keyword list is silently lost. Surfaced as the
        // `inv-org-render-fixed-point` PBT flake (May 2026, see
        // `devlog/2026-05-19-phase-c-validation-diagnosis.md`).
        self.command_bus
            .execute_operation_with_origin(
                &EntityName::new("block"),
                "update",
                params,
                EventOrigin::Org,
            )
            .await
            .map_err(|e| anyhow::anyhow!("{}", e))?;
        // Update in-memory cache
        self.live.insert(doc.id.as_str().to_string(), doc.clone());
        Ok(())
    }
}

/// AliasRegistrar backed by LoroDocumentStore.
///
/// Must share the same `Arc<RwLock<LoroDocumentStore>>` as LoroBlockReader/LoroBlockOperations.
pub struct LoroAliasRegistrar {
    doc_store: Arc<tokio::sync::RwLock<LoroDocumentStore>>,
}

#[async_trait::async_trait]
impl crate::org_sync_controller::AliasRegistrar for LoroAliasRegistrar {
    async fn register_alias(&self, doc_id: &EntityUri, path: &Path) {
        let store = self.doc_store.read().await;
        store.register_alias(doc_id.as_str(), path).await;
    }

    async fn resolve_alias_to_path(&self, doc_id: &EntityUri) -> Option<PathBuf> {
        let store = self.doc_store.read().await;
        store.resolve_alias_to_path(doc_id.as_str()).await
    }
}

/// Configuration for OrgMode integration
#[derive(Clone, Debug)]
pub struct OrgModeConfig {
    /// Root directory containing .org files
    pub root_directory: PathBuf,
    /// Directory where .loro files are stored (legacy, used when Loro is managed by OrgMode)
    pub loro_storage_dir: PathBuf,
    /// Debounce window in milliseconds for OrgSyncController.
    /// Events are batched and rendered after this quiet period.
    pub debounce_ms: u64,
    /// Shell command to run after each org file write (e.g. "jj new").
    /// Runs in root_directory with HOLON_FILE env var set to the written path.
    pub post_org_write_hook: Option<String>,
}

impl OrgModeConfig {
    pub fn new(root_directory: PathBuf) -> Self {
        // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        // This ensures path comparisons work correctly when file watcher reports
        // canonicalized paths
        let root_directory = std::fs::canonicalize(&root_directory).unwrap_or(root_directory);
        let loro_storage_dir = root_directory.join(".loro");
        Self {
            root_directory,
            loro_storage_dir,
            debounce_ms: 500,
            post_org_write_hook: None,
        }
    }

    pub fn with_loro_storage(root_directory: PathBuf, loro_storage_dir: PathBuf) -> Self {
        // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        let root_directory = std::fs::canonicalize(&root_directory).unwrap_or(root_directory);
        let loro_storage_dir = std::fs::canonicalize(&loro_storage_dir).unwrap_or(loro_storage_dir);
        Self {
            root_directory,
            loro_storage_dir,
            debounce_ms: 500,
            post_org_write_hook: None,
        }
    }
}

/// ServiceModule for OrgMode integration
///
/// Registers OrgMode-specific services in the DI container.
/// Loro services are NOT registered here — they come from LoroModule (if enabled).
///
/// OrgMode will detect if LoroBlockOperations is available in DI and use it;
/// otherwise it falls back to SqlOperationProvider.
pub struct OrgModeModule;

impl Module for OrgModeModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        use tracing::{error, info};

        info!("[OrgModeModule] register_services called");

        // Create and register FileWatcherReadySignal
        // Tests can wait on this to ensure file watcher is ready before external mutations
        let (ready_sender, ready_signal) = FileWatcherReadySignal::new();
        let ready_signal = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_signal)));
        injector.provide::<FileWatcherReadySignal>(Provider::root(move |_| {
            let signal = ready_signal
                .lock()
                .unwrap()
                .take()
                .expect("FileWatcherReadySignal factory called twice");
            Shared::new(signal)
        }));
        // Store sender in Arc<Mutex> so we can move it into the spawned task later
        let ready_sender = std::sync::Arc::new(std::sync::Mutex::new(Some(ready_sender)));
        let ready_sender_for_factory = ready_sender.clone();

        // Create and register OrgSyncIdleSignal
        // Tests use this to skip mtime polling on the hot path.
        let idle_signal = OrgSyncIdleSignal::new();
        let idle_signal_for_factory = idle_signal.clone();
        injector
            .provide::<OrgSyncIdleSignal>(Provider::root(move |_| idle_signal_for_factory.clone()));
        let idle_signal_for_loop = idle_signal;

        // Register OrgModeSyncProvider as a factory
        injector.provide::<OrgModeSyncProvider>(Provider::root_async(async |resolver| {
            let config = resolver.resolve::<OrgModeConfig>();
            let token_store = resolver
                .try_resolve_async::<dyn SyncTokenStore>()
                .await
                .expect("[OrgModeModule] SyncTokenStore not found in DI");
            Shared::new(OrgModeSyncProvider::new(
                config.root_directory.clone(),
                token_store,
            ))
        }));

        // Register SyncableProvider trait implementation
        injector.provide_into_set::<dyn SyncableProvider>(Provider::root(|resolver| {
            let sync_provider = resolver.resolve::<OrgModeSyncProvider>();
            sync_provider.clone() as Arc<dyn SyncableProvider>
        }));

        // Register filesystem entity types in the TypeRegistry for GQL graph.
        // Done inside an async provider so TypeRegistry is already available.
        injector.provide::<QueryableCache<Directory>>(Provider::root_async(|r| async move {
            let type_registry = r.resolve::<TypeRegistry>();
            if let Err(e) = type_registry.register(Directory::type_definition()) {
                tracing::warn!("[OrgModeModule] Failed to register Directory type: {e}");
            }
            Shared::new(holon::di::create_queryable_cache_async(&r).await)
        }));
        injector.provide::<QueryableCache<File>>(Provider::root_async(|r| async move {
            let type_registry = r.resolve::<TypeRegistry>();
            if let Err(e) = type_registry.register(File::type_definition()) {
                tracing::warn!("[OrgModeModule] Failed to register File type: {e}");
            }
            Shared::new(holon::di::create_queryable_cache_async(&r).await)
        }));

        // TursoEventBus is registered by FrontendConfig shared infrastructure

        // Register OrgRenderer
        injector.provide::<OrgRenderer>(Provider::root(|_resolver| Shared::new(OrgRenderer)));

        // Set up event bus wiring and background tasks.
        // This factory resolves LoroBlockOperations if available (Loro enabled),
        // otherwise creates a SqlOperationProvider for direct SQL block operations.
        injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(move |resolver| {
            let ready_sender_clone = ready_sender_for_factory.clone();
            let idle_signal_clone = idle_signal_for_loop.clone();
            async move {
            // ============================================================
            // PHASE 1: Resolve ALL services that run DDL
            // This ensures all schema initialization completes BEFORE
            // any background tasks start using the database.
            // ============================================================
            info!("[OrgMode] Phase 1: Resolving services (DDL)");

            let _dir_cache = resolver.resolve_async::<QueryableCache<Directory>>().await;
            let _file_cache = resolver.resolve_async::<QueryableCache<File>>().await;
            let _block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;
            let sync_provider = resolver.resolve_async::<OrgModeSyncProvider>().await;

            // IMPORTANT: Resolve TursoEventBus HERE, not after spawns!
            // TursoEventBus::init_schema() runs DDL that must complete
            // before any background tasks use the database.
            let event_bus = resolver.resolve_async::<TursoEventBus>().await;
            let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();

            // Resolve remaining services
            let config = resolver.resolve::<OrgModeConfig>();

            // Try to resolve Loro services (available if LoroModule was registered)
            // ALLOW(ok): optional DI service
            let loro_ops: Option<Arc<LoroBlockOperations>> =
                resolver.try_resolve::<LoroBlockOperations>().ok();

            let loro_available = loro_ops.is_some();
            info!(
                "[OrgMode] Phase 1 complete: All DDL finished (loro={})",
                loro_available
            );

            // Resolve DbHandle unconditionally — Turso is always available
            let db_handle_provider =
                resolver.resolve::<dyn holon::di::DbHandleProvider>();
            let db_handle = db_handle_provider.handle();

            // OrgSyncController writes through SQL ops; CacheBlockReader reads from QueryableCache
            // which is also backed by the same Turso database, ensuring consistency.
            let sql_ops = Arc::new(holon::core::SqlOperationProvider::with_event_bus_and_edge_fields(
                db_handle.clone(),
                BLOCK_WRITE_TABLE.to_string(),
                "block".to_string(),
                "block".to_string(),
                event_bus_arc.clone(),
                BlockSchemaModule.edge_fields(),
            ));

            let command_bus: Arc<dyn OperationProvider> =
                sql_ops.clone() as Arc<dyn OperationProvider>;

            // ============================================================
            // PHASE 2: Create OrgSyncController
            // Single controller using last_projection for echo suppression.
            // ============================================================
            info!("[OrgMode] Phase 2: Creating OrgSyncController");

            info!("[OrgMode] Phase 2 complete");

            // ============================================================
            // PHASE 3: Spawn background tasks
            // The DatabaseActor serializes all operations, eliminating race conditions
            // between DDL and DML operations.
            // ============================================================
            info!("[OrgMode] Phase 3: Spawning background tasks");

            // NOTE: Direct cache writes (Task 1) removed. All block writes now go
            // through EventBus (via OrgSyncController → command_bus → EventBus).
            // Directory and file changes still go through OrgModeEventAdapter → EventBus.

            // Initial sync task
            // The DatabaseActor serializes all operations, eliminating race conditions.
            {
                let sync_provider_clone = sync_provider.clone();
                tokio::spawn(async move {
                    use holon::core::datasource::SyncableProvider;
                    if let Err(e) = sync_provider_clone
                        .sync(holon::core::datasource::StreamPosition::Beginning)
                        .await
                    {
                        error!("[OrgMode] Initial sync failed: {}", e);
                    }
                });
            }

            // Loro ↔ command/event bus is wired by `LoroModule` via
            // `LoroSyncControllerHandle`; see `crates/holon/src/sync/loro_module.rs`.

            // OrgModeSyncProvider → EventBus (directories and files only)
            {
                let sync_provider_clone = sync_provider.clone();
                let event_bus_clone = event_bus_arc.clone();
                let error_tracker = resolver.try_resolve::<PublishErrorTracker>()
                    .map(|t| (*t).clone())
                    .unwrap_or_else(|_| PublishErrorTracker::new());
                tokio::spawn(async move {
                    let adapter =
                        OrgModeEventAdapter::with_error_tracker(event_bus_clone, error_tracker);
                    let dir_rx = sync_provider_clone.subscribe_directories();
                    let file_rx = sync_provider_clone.subscribe_files();
                    if let Err(e) = adapter.start(dir_rx, file_rx) {
                        error!("[OrgMode] Failed to start OrgModeEventAdapter: {}", e);
                    }
                });
            }

            // OrgSyncController: unified file ↔ block sync
            // Subscribes to EventBus for block events (works with both Loro and SQL paths).
            // Runs on a single task via tokio::select!, serializing
            // on_file_changed and on_block_changed — no locks needed.
            {
                let command_bus = command_bus.clone();
                let config_clone = config.clone();
                let event_bus_for_ctrl = event_bus_arc.clone();
                let ready_sender_clone = ready_sender_clone.clone();
                // Weak reference to detect session shutdown: when the
                // injector + FrontendSession drop their strong refs,
                // upgrade() returns None and the file-watcher loop exits.
                // Without this, shared-runtime PBT (sut.rs:5320) leaks one
                // file-watcher per case → poll rate climbs from 9 → 120 Hz.
                let idle_signal_weak = std::sync::Arc::downgrade(&idle_signal_clone);

                let loro_ops_clone = loro_ops.clone();
                let block_cache = resolver.resolve_async::<QueryableCache<Block>>().await;
                let ordering = resolver.resolve_async::<dyn BlockOrdering>().await;
                // Downstream consolidator→sink projection. Present only when a
                // separate consolidator owns block storage (registered by
                // LoroModule); absent in the degraded SQL-only config, where
                // the controller routes creates through the command bus.
                let downstream = resolver
                    .optional_resolve_async::<dyn holon_core::DownstreamProjection>()
                    .await;
                let backend_provider =
                    resolver.resolve::<dyn holon::di::TursoBackendProvider>();
                let backend_for_live_docs = backend_provider.backend();
                let command_bus_for_docs = command_bus.clone();

                tokio::spawn(async move {
                    use tracing::Instrument;
                    let doc_manager: Arc<dyn DocumentManager> = Arc::new(
                        async {
                            LiveDocumentManager::new(command_bus_for_docs, backend_for_live_docs)
                                .await
                                .expect("Failed to create LiveDocumentManager")
                        }
                        .instrument(tracing::info_span!("org.startup.live_doc_manager_new"))
                        .await,
                    );

                    let block_reader: Arc<dyn BlockReader> = Arc::new(
                        CacheBlockReader::new(block_cache)
                            .with_event_bus(event_bus.clone()),
                    );

                    let mut controller = OrgSyncController::new(
                        block_reader,
                        doc_manager,
                        config_clone.root_directory.clone(),
                        ordering,
                    );

                    if let Some(hook_cmd) = config_clone.post_org_write_hook.clone() {
                        controller = controller.with_post_org_write_hook(hook_cmd);
                    }

                    if let Some(downstream) = downstream {
                        controller = controller.with_downstream_projection(downstream);
                    }

                    if let Some(ref ops) = loro_ops_clone {
                        let shared_doc_store = ops.shared_doc_store();
                        let alias_registrar: Arc<dyn crate::org_sync_controller::AliasRegistrar> =
                            Arc::new(LoroAliasRegistrar { doc_store: shared_doc_store });
                        controller = controller.with_alias_registrar(alias_registrar);
                    }

                    let init_result = async { controller.initialize().await }
                        .instrument(tracing::info_span!("org.startup.controller_initialize"))
                        .await;
                    if let Err(e) = init_result {
                        let msg = format!("OrgSyncController initialization failed: {}", e);
                        error!("[OrgMode] {}", msg);
                        if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                            sender.signal_error(msg);
                        }
                        return;
                    }

                    let block_filter = holon::sync::event_bus::EventFilter::new()
                        .with_aggregate_type(holon::sync::event_bus::AggregateType::Block)
                        .with_status(holon::sync::event_bus::EventStatus::Confirmed);
                    let subscribe_result = async {
                        event_bus_for_ctrl
                            .subscribe(block_filter, holon::sync::event_bus::Consumer::ORG)
                            .await
                    }
                    .instrument(tracing::info_span!("org.startup.event_bus_subscribe"))
                    .await;
                    let mut event_rx = match subscribe_result {
                        Ok(rx) => rx,
                        Err(e) => {
                            let msg = format!("Failed to subscribe to EventBus: {}", e);
                            error!("[OrgMode] {}", msg);
                            if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                sender.signal_error(msg);
                            }
                            return;
                        }
                    };

                    // Build watcher infra without registering the recursive
                    // watch yet — the slow `notify::watch()` call (9+s on
                    // macOS) is deferred until after signal_ready so the
                    // factory can return immediately.
                    let watcher_result =
                        tracing::info_span!("org.startup.file_watcher_new_unarmed")
                            .in_scope(|| OrgFileWatcher::new_unarmed(&config_clone.root_directory));
                    match watcher_result {
                        Ok(watcher) => {
                            info!(
                                "[OrgMode] File watcher built (unarmed) for: {}",
                                config_clone.root_directory.display()
                            );

                            use tracing::Instrument;

                            // Split out file_rx and the bare RecommendedWatcher.
                            // The notify_watcher must stay alive (its callback
                            // pushes events into file_rx); the slow arm step
                            // is deferred to a background task while the main
                            // loop runs.
                            let (notify_watcher, mut file_rx, _hashes) =
                                watcher.into_parts();

                            // Initial scan ingests pre-existing files BEFORE
                            // signal_ready so prime_seed_count's expected
                            // block count can match immediately.
                            //
                            // Per-file failures are collected and propagated
                            // through the ReadySignal — swallowing them at
                            // ERROR-log level left downstream consumers
                            // (LiveData mirrors, matview cursors) wedged
                            // because partial-state writes never reconciled.
                            let scan_failures: Vec<(std::path::PathBuf, anyhow::Error)> = async {
                                let org_files =
                                    scan_org_files(&config_clone.root_directory);
                                let preloaded: Vec<(std::path::PathBuf, Option<String>)> =
                                    futures::future::join_all(
                                        org_files.into_iter().map(|p| async move {
                                            let content =
                                                tokio::fs::read_to_string(&p).await.ok(); // ALLOW(ok): best-effort OS page-cache warmup; content is dropped below
                                            (p, content)
                                        }),
                                    )
                                    .instrument(tracing::info_span!(
                                        "org.initial_scan.parallel_read"
                                    ))
                                    .await;
                                let mut failures = Vec::new();
                                for (file_path, _content) in preloaded {
                                    if let Err(e) =
                                        controller.on_file_changed(&file_path).await
                                    {
                                        error!(
                                            "[OrgMode] Failed to process existing file {}: {}",
                                            file_path.display(),
                                            e
                                        );
                                        failures.push((file_path, e));
                                    }
                                }
                                failures
                            }
                            .instrument(tracing::info_span!("org.initial_scan.ingest"))
                            .await;

                            // Project rule: fail loud, never fake. Any
                            // initial-scan failure propagates as a startup
                            // error — silently continuing past it hides
                            // upstream bugs (e.g. Loro inbound runtime not
                            // mirroring org-ingested blocks, surfacing later
                            // as `update_block_position`/`resolve_parent_tree_id`
                            // failures).
                            if !scan_failures.is_empty() {
                                let summary = scan_failures
                                    .iter()
                                    .map(|(p, e)| format!("{}: {}", p.display(), e))
                                    .collect::<Vec<_>>()
                                    .join("; ");
                                let msg = format!(
                                    "OrgMode initial scan failed for {} file(s): {}",
                                    scan_failures.len(),
                                    summary
                                );
                                error!("[OrgMode] {}", msg);
                                if let Some(sender) =
                                    ready_sender_clone.lock().unwrap().take()
                                {
                                    sender.signal_error(msg);
                                }
                                return;
                            }

                            // Phase 1 fix: signal_ready BEFORE arm(). The
                            // 9+ s `notify::watch(Recursive)` on macOS runs
                            // detached in the background. Correctness during
                            // the unarmed window is preserved by
                            // `poll_external_changes`, which now also walks
                            // the tree to discover new files via
                            // `scan_directory` (see org_sync_controller.rs).
                            // Without that Phase A→B extension, this fix
                            // breaks `create_document`.
                            if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                sender.signal_ready();
                            }

                            // Spawn arm() on the blocking pool, detached.
                            // Owns the notify_watcher and holds it alive
                            // forever via `pending::<()>().await` — dropping
                            // the RecommendedWatcher silently stops event
                            // delivery into `file_rx`. AbortOnDrop wraps the
                            // JoinHandle so this task terminates when the
                            // outer file-watcher loop exits via the
                            // Weak<OrgSyncIdleSignal> shutdown.
                            let dir_for_arm = config_clone.root_directory.clone();
                            let arm_task = tokio::spawn(
                                async move {
                                    let r = tokio::task::spawn_blocking(move || {
                                        use notify::Watcher;
                                        let mut nw = notify_watcher;
                                        let r = nw.watch(
                                            &dir_for_arm,
                                            notify::RecursiveMode::Recursive,
                                        );
                                        (nw, r)
                                    })
                                    .await;
                                    match r {
                                        Ok((nw, Ok(()))) => {
                                            info!("[OrgMode] watcher armed");
                                            let _kept = nw;
                                            std::future::pending::<()>().await;
                                        }
                                        Ok((_, Err(e))) => {
                                            error!(
                                                "[OrgMode] watch_recursive failed: {}",
                                                e
                                            );
                                        }
                                        Err(e) => {
                                            error!(
                                                "[OrgMode] arm spawn_blocking panicked: {}",
                                                e
                                            );
                                        }
                                    }
                                }
                                .instrument(tracing::info_span!(
                                    "org.startup.arm_watcher_blocking"
                                )),
                            );
                            struct AbortOnDrop(tokio::task::JoinHandle<()>);
                            impl Drop for AbortOnDrop {
                                fn drop(&mut self) {
                                    self.0.abort();
                                }
                            }
                            let _arm_keepalive = AbortOnDrop(arm_task);


                            // Main loop: handle file changes and EventBus block events.
                            //
                            // Two periodic tickers backstop the notify-driven
                            // `file_rx` path:
                            //
                            // - `poll_tick` (100ms): re-stats every tracked
                            //   `last_projection` entry. Cheap — short-circuited
                            //   by an `(mtime, size)` signature so unchanged
                            //   files don't read.
                            // - `discovery_tick` (2s): walks the full tree via
                            //   `scan_directory` to pick up files created during
                            //   notify's unarmed window on macOS. Expensive
                            //   (rebuilds `ignore::WalkBuilder` gitignore DFAs)
                            //   so deliberately infrequent.
                            //
                            // Missed FSEvents now become a 100ms latency blip
                            // for modifications and ≤2s for brand-new files.
                            let mut poll_tick = tokio::time::interval(
                                tokio::time::Duration::from_millis(100),
                            );
                            poll_tick.set_missed_tick_behavior(
                                tokio::time::MissedTickBehavior::Skip,
                            );
                            let mut discovery_tick = tokio::time::interval(
                                tokio::time::Duration::from_secs(2),
                            );
                            discovery_tick.set_missed_tick_behavior(
                                tokio::time::MissedTickBehavior::Skip,
                            );
                            // Coalesce mark_processed across bursty events; flushes
                            // every 2ms (well under wait_for_consumers' 1ms→100ms
                            // poll backoff) or when the stream closes.
                            let mut pending_marks: Vec<holon::sync::event_bus::EventId> = Vec::new();
                            let mut mark_flush_tick = tokio::time::interval(
                                tokio::time::Duration::from_millis(2),
                            );
                            mark_flush_tick.set_missed_tick_behavior(
                                tokio::time::MissedTickBehavior::Delay,
                            );

                            // Coalesce orphan-event full re-renders. Events that
                            // lack routing_doc_uri (and whose payload parent_id
                            // doesn't resolve via on_block_changed) used to trigger
                            // re_render_all_tracked per event — O(events × tracked
                            // files) IO + segment-chain lookups during bursty
                            // initial scans. The flag is set in the event arm; a
                            // 50ms ticker drains it with a single re-render pass.
                            let mut pending_full_rerender = false;
                            let mut rerender_flush_tick = tokio::time::interval(
                                tokio::time::Duration::from_millis(50),
                            );
                            rerender_flush_tick.set_missed_tick_behavior(
                                tokio::time::MissedTickBehavior::Skip,
                            );
                            loop {
                                // Session-alive check: if the strong refs to
                                // OrgSyncIdleSignal have all been dropped, the
                                // owning FrontendSession is gone — exit.
                                let Some(idle_signal_for_task) = idle_signal_weak.upgrade() else {
                                    info!("[OrgMode] file-watcher loop exiting (session dropped)");
                                    return;
                                };
                                tokio::select! {
                                    Some(file_path) = file_rx.recv() => {
                                        tracing::debug!("[ORGSYNC_TRACE] file_rx -> on_file_changed({})", file_path.display());
                                        if let Err(e) = controller.on_file_changed(&file_path).await {
                                            tracing::debug!(
                                                "[ORGSYNC_TRACE] on_file_changed ERROR for {}: {}",
                                                file_path.display(), e
                                            );
                                            error!(
                                                "[OrgMode] File change error {}: {}",
                                                file_path.display(), e
                                            );
                                        } else {
                                            tracing::debug!("[ORGSYNC_TRACE] on_file_changed OK for {}", file_path.display());
                                        }
                                        idle_signal_for_task.mark_progress();
                                    }
                                    _ = poll_tick.tick() => {
                                        match controller.poll_tracked_files().await {
                                            Ok(n) if n > 0 => {
                                                tracing::debug!("[ORGSYNC_TRACE] poll ingested {} file(s)", n);
                                                idle_signal_for_task.mark_progress();
                                            }
                                            Ok(_) => {}
                                            Err(e) => {
                                                tracing::debug!("[ORGSYNC_TRACE] poll ERROR: {}", e);
                                                error!("[OrgMode] poll_tracked_files error: {}", e);
                                            }
                                        }
                                    }
                                    _ = discovery_tick.tick() => {
                                        match controller.poll_new_files().await {
                                            Ok(n) if n > 0 => {
                                                tracing::debug!("[ORGSYNC_TRACE] discovery ingested {} new file(s)", n);
                                                idle_signal_for_task.mark_progress();
                                            }
                                            Ok(_) => {}
                                            Err(e) => {
                                                tracing::debug!("[ORGSYNC_TRACE] discovery ERROR: {}", e);
                                                error!("[OrgMode] poll_new_files error: {}", e);
                                            }
                                        }
                                    }
                                    Some(event) = tokio_stream::StreamExt::next(&mut event_rx) => {
                                        use tracing::Instrument;
                                        use tracing_opentelemetry::OpenTelemetrySpanExt;
                                        let event_id = event.id.clone();
                                        let span = tracing::info_span!(
                                            "org.on_event",
                                            event_id = %event.id,
                                            event_kind = ?event.event_kind,
                                            aggregate_id = %event.aggregate_id,
                                            trace_id = ?event.trace_id,
                                        );
                                        let _ = span.set_parent(holon::sync::event_bus::parent_context_from_event(
                                            event.trace_id.as_deref(),
                                            event.span_id.as_deref(),
                                            event.trace_flags,
                                        ));
                                        async {
                                            let doc_ids = extract_doc_ids_from_event(&event);
                                            if doc_ids.is_empty() {
                                                tracing::debug!(
                                                    "[OrgMode] Block event {} ({:?}) missing routing_doc_uri — queued for batched re-render",
                                                    event.aggregate_id, event.event_kind,
                                                );
                                                pending_full_rerender = true;
                                            } else {
                                                let mut any_routed = false;
                                                for doc_id in &doc_ids {
                                                    match controller.on_block_changed(doc_id).await {
                                                        Ok(true) => { any_routed = true; }
                                                        Ok(false) => {}
                                                        Err(e) => {
                                                            error!(
                                                                "[OrgMode] Block change error for {}: {}",
                                                                doc_id, e
                                                            );
                                                        }
                                                    }
                                                }
                                                if !any_routed {
                                                    // ALLOW(fallback): disclosed re-render path when event lacked routing_doc_uri
                                                    pending_full_rerender = true;
                                                }
                                            }
                                        }.instrument(span).await;
                                        pending_marks.push(event_id);
                                        idle_signal_for_task.mark_progress();
                                    }
                                    _ = mark_flush_tick.tick(), if !pending_marks.is_empty() => {
                                        let ids = std::mem::take(&mut pending_marks);
                                        if let Err(e) = event_bus_for_ctrl.mark_processed_batch(&ids, holon::sync::event_bus::Consumer::ORG).await {
                                            tracing::warn!(
                                                "[OrgMode] mark_processed_batch(org, {}) failed: {}",
                                                ids.len(), e
                                            );
                                        }
                                    }
                                    _ = rerender_flush_tick.tick(), if pending_full_rerender => {
                                        pending_full_rerender = false;
                                        if let Err(e) = controller.re_render_all_tracked().await {
                                            error!("[OrgMode] re_render_all_tracked (debounced) error: {}", e);
                                        }
                                    }
                                }
                            }
                        }
                        Err(e) => {
                            let msg = format!("Failed to start file watcher: {}", e);
                            error!("[OrgMode] {}", msg);
                            if let Some(sender) = ready_sender_clone.lock().unwrap().take() {
                                sender.signal_error(msg);
                            }
                        }
                    }
                });
            }

            // Always use SQL ops for write path consistency with QueryableCache reads.
            let wrapper =
                OperationWrapper::new(sql_ops.clone(), Some(sync_provider));
            Arc::new(wrapper) as Arc<dyn OperationProvider>
        }}));

        Ok(())
    }
}

/// Extract unique document IDs from an EventBus event.
///
/// For block.created/block.updated events, we look at the block's parent_id in the payload.
/// Document IDs are identified by the "doc:" URI scheme.
fn extract_doc_ids_from_event(event: &holon::sync::event_bus::Event) -> Vec<EntityUri> {
    use holon::sync::event_bus::EventKind;
    use std::collections::HashSet;

    let mut doc_ids = HashSet::new();

    match event.event_kind {
        EventKind::Created | EventKind::Updated | EventKind::Deleted | EventKind::FieldsChanged => {
            // Typed `Event::routing_doc_uri` field is the primary source —
            // SqlOperationProvider sets it at the operation boundary so we
            // don't have to hunt through `payload` for the underscore-prefixed
            // hint.
            if let Some(doc_uri) = event.routing_doc_uri.as_deref() {
                if let Ok(uri) = holon_api::EntityUri::parse(doc_uri) {
                    doc_ids.insert(uri);
                }
            }
            // Fall back to parent_id in data (for events lacking routing —
            // e.g. Loro outbound batched creates that don't go through
            // `find_document_uri` at the boundary).
            if doc_ids.is_empty() {
                if let Some(data) = event.payload.get("data") {
                    if let Some(parent_id) = data.get("parent_id").and_then(|v| v.as_str()) {
                        if let Ok(uri) = holon_api::EntityUri::parse(parent_id) {
                            doc_ids.insert(uri);
                        }
                    }
                }
            }
        }
    }

    doc_ids.into_iter().collect()
}

/// Extension trait for registering OrgMode services in a [`Injector`]
///
/// This trait provides a convenient method to register all OrgMode-related
/// services with a single call, taking just the root directory as a parameter.
///
/// # Example
///
/// ```rust,ignore
/// use holon_orgmode::di::OrgModeInjectorExt;
/// use std::path::PathBuf;
///
/// // In your DI setup closure:
/// services.add_orgmode(PathBuf::from("/path/to/org/files"))?;
/// ```
pub trait OrgModeInjectorExt {
    fn add_orgmode(&self, root_directory: PathBuf) -> std::result::Result<(), fluxdi::Error>;
}

impl OrgModeInjectorExt for Injector {
    fn add_orgmode(&self, root_directory: PathBuf) -> std::result::Result<(), fluxdi::Error> {
        self.provide::<OrgModeConfig>(Provider::root(move |_| {
            Shared::new(OrgModeConfig::new(root_directory.clone()))
        }));
        OrgModeModule.configure(self)?;
        Ok(())
    }
}

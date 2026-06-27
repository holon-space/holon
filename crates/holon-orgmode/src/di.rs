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

use crate::file_sync_controller::FileSyncController;
use crate::file_watcher::OrgFileWatcher;
use crate::org_renderer::OrgRenderer;
use crate::traits::{BlockReader, DocumentManager};
use crate::OrgModeSyncProvider;
use holon::core::datasource::{
    OperationProvider, OriginTaggedWrites, SyncTokenStore, SyncableProvider,
};
use holon::core::operation_wrapper::OperationWrapper;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::schema_module::SchemaModule;
use holon::storage::schema_modules::BlockSchemaModule;
use holon::storage::{BLOCK_READ_TABLE, BLOCK_WRITE_TABLE};
use holon::sync::{LoroBlockOperations, LoroDocumentStore};
use holon::type_registry::TypeRegistry;
use holon_api::block::{blocks_by_document, Block};
use holon_api::{EntityName, EntityUri};
use holon_core::block_ordering::BlockOrdering;
use holon_core::EventOrigin;

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

    /// Consume the wrapper and return the inner watch receiver so
    /// downstream consumers (e.g. holon-frontend) can call `borrow()`
    /// without depending on `holon-orgmode`.
    pub fn into_receiver(self) -> tokio::sync::watch::Receiver<Option<Result<(), String>>> {
        self.receiver
    }

    /// Check if startup has completed (either success or failure).
    pub fn is_completed(&self) -> bool {
        self.receiver.borrow().is_some()
    }

    /// Wait until the file watcher signals readiness.
    ///
    /// Returns `Ok(())` on success, `Err` if the FileSyncController startup failed.
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
            Err(msg) => Err(anyhow::anyhow!(
                "FileSyncController startup failed: {}",
                msg
            )),
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

/// Event-driven idle signal for the FileSyncController loop.
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
    /// Highest fully-processed `FileChange.seq` (ADR 0011). Advanced by the
    /// controller loop after each change — forwarded ones after
    /// `on_file_changed` returns, filtered ones immediately — strictly in
    /// delivery order, so `seq <= watermark` means "that change (and every
    /// earlier one) has been processed".
    change_seq: std::sync::atomic::AtomicU64,
}

impl OrgSyncIdleSignal {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            tick: std::sync::atomic::AtomicU64::new(0),
            notify: tokio::sync::Notify::new(),
            change_seq: std::sync::atomic::AtomicU64::new(0),
        })
    }

    /// Current tick value. Increases monotonically.
    pub fn current_tick(&self) -> u64 {
        self.tick.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Advance the processed-change watermark (monotonic) and wake waiters.
    pub fn advance_change_seq(&self, seq: u64) {
        self.change_seq
            .fetch_max(seq, std::sync::atomic::Ordering::Release);
        self.notify.notify_waiters();
    }

    /// Highest fully-processed `FileChange.seq`.
    pub fn processed_change_seq(&self) -> u64 {
        self.change_seq.load(std::sync::atomic::Ordering::Acquire)
    }

    /// Wait until the processed-change watermark reaches `seq`, or `timeout`
    /// elapses. Returns `true` when the watermark was reached. Deterministic
    /// counterpart to [`wait_quiescent`] for changes whose seq the caller
    /// knows (e.g. an in-memory write it just made).
    ///
    /// [`wait_quiescent`]: Self::wait_quiescent
    pub async fn wait_for_change_seq(&self, seq: u64, timeout: std::time::Duration) -> bool {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            // Subscribe BEFORE checking to avoid missing a wake.
            let notified = self.notify.notified();
            if self.processed_change_seq() >= seq {
                return true;
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return false;
            }
            let _ = tokio::time::timeout(remaining, notified).await;
        }
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
/// Delegates to `file_watcher::scan_directory` — the gitignore-aware walk
/// behind the `FileSystem` port (ADR 0011), filtered to `.org`.
async fn scan_org_files(
    fs: &dyn holon_filesystem::FileSystem,
    dir: &std::path::Path,
) -> std::io::Result<Vec<PathBuf>> {
    Ok(crate::file_watcher::scan_directory(fs, dir).await?.files)
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
    /// Phase 5 keystone: the convergent block feed, drives
    /// `wait_for_blocks_in_feed` (the positional cache catch-up that replaced
    /// the `event_acks` watermark wait). `None` for backends without a feed.
    block_feed: Option<Arc<holon::sync::LiveData<Block>>>,
}

impl CacheBlockReader {
    pub fn new(cache: Arc<QueryableCache<Block>>) -> Self {
        Self {
            cache,
            block_feed: None,
        }
    }

    /// Phase 5 keystone: wire the convergent `LiveData<Block>` feed so
    /// `wait_for_blocks_in_feed` can prove the positional catch-up condition.
    pub fn with_block_feed(mut self, block_feed: Arc<holon::sync::LiveData<Block>>) -> Self {
        self.block_feed = Some(block_feed);
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
             FROM {BLOCK_WRITE_TABLE} b \
             ORDER BY b.sort_key, b.id"
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
            JOIN descendants d ON d.id = b.id \
            ORDER BY b.sort_key, b.id",
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

    async fn wait_for_blocks_in_feed(&self, block_ids: &[String], timeout_ms: u64) -> bool {
        match &self.block_feed {
            Some(feed) => {
                let ids: Vec<String> = block_ids.to_vec();
                feed.wait_until(
                    move |m| ids.iter().all(|id| m.contains_key(id)),
                    std::time::Duration::from_millis(timeout_ms),
                )
                .await
            }
            None => true,
        }
    }

    async fn persist_file_hash(
        &self,
        file_id: &holon_api::EntityUri,
        hash: &str,
    ) -> anyhow::Result<()> {
        let params = vec![
            holon_api::Value::String(hash.to_string()),
            holon_api::Value::String(file_id.to_string()),
        ];
        self.cache
            .db_handle()
            .execute_values("UPDATE file SET content_hash = ? WHERE id = ?", params)
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
    command_bus: Arc<dyn OriginTaggedWrites>,
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
        command_bus: Arc<dyn OriginTaggedWrites>,
        db_handle: holon::storage::DbHandle,
    ) -> anyhow::Result<Self> {
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
            .map(|d| (**d).clone()))
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
        // it tells FileSyncController which file to re-render.
        let params = build_block_params(&doc, &doc.parent_id, &doc.id);
        // INSERT OR IGNORE: only triggers on PK collision now that the
        // partial unique index on `(parent_id, name)` is gone. The
        // `create_lock` above is what prevents same-title duplicates.
        // Tag the create event with `EventOrigin::Org` so the
        // `LoroSyncController` inbound gate routes it to `Apply` instead of
        // dropping it as a generic SQL-direct write. This page-creation flow
        // is triggered by `FileSyncController::on_file_changed`; semantically
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
            // ALLOW(entity_uri_from_raw): existing_id from command_bus execute_operation response (SQL Value::String)
            let existing_uri = EntityUri::from_raw(&existing_id);
            if let Some(existing) = self.get_by_id(&existing_uri).await? {
                return Ok(existing);
            }
            // The document exists in SQL but not in LiveData.
            // Insert it so subsequent find_by_parent_and_name / get_by_id lookups succeed.
            let mut existing_doc = doc.clone();
            existing_doc.id = existing_uri;
            self.live.insert(
                existing_doc.id.as_str().to_string(),
                Arc::new(existing_doc.clone()),
            );
            return Ok(existing_doc);
        }

        self.live
            .insert(doc.id.as_str().to_string(), Arc::new(doc.clone()));
        Ok(doc)
    }

    async fn get_by_id(&self, id: &EntityUri) -> anyhow::Result<Option<Block>> {
        let docs = self.live.read();
        Ok(docs.get(id.as_str()).map(|d| (**d).clone()))
    }

    async fn update_metadata(&self, doc: &Block) -> anyhow::Result<()> {
        use crate::block_params::build_block_params;
        let mut params = build_block_params(doc, &doc.parent_id, &doc.id);
        // `doc.properties` is authoritative for doc-level metadata, but
        // `build_block_params` only emits keys that are PRESENT — and the
        // SQL provider's property merge can't clear what isn't mentioned.
        // Emit a `Value::Null` removal sentinel for every property the
        // previously-known doc had that the new doc no longer carries
        // (e.g. `todo_keywords` after the `#+TODO:` header was deleted).
        if let Some(old) = self.live.read().get(doc.id.as_str()).cloned() {
            for key in old.properties.keys() {
                if !doc.properties.contains_key(key) && !params.contains_key(key.as_str()) {
                    params.insert(key.as_str().into(), holon_api::Value::Null);
                }
            }
        }
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
        self.live
            .insert(doc.id.as_str().to_string(), Arc::new(doc.clone()));
        Ok(())
    }
}

/// AliasRegistrar backed by LoroDocumentStore.
///
/// Must share the same `Arc<RwLock<LoroDocumentStore>>` as LoroBlockReader/LoroBlockOperations.
pub struct LoroAliasRegistrar {
    pub doc_store: Arc<tokio::sync::RwLock<LoroDocumentStore>>,
}

#[async_trait::async_trait]
impl crate::file_sync_controller::AliasRegistrar for LoroAliasRegistrar {
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
    /// Shell command to run after each org file write (e.g. "jj new").
    /// Runs in root_directory with HOLON_FILE env var set to the written path.
    pub post_org_write_hook: Option<String>,
    /// `(filename, content)` documents seeded through the `FileSystem` port
    /// when the vault contains no .org files (ADR 0011). Filled by the app
    /// wiring from `holon_frontend::DEFAULT_ASSETS`; empty = no seeding.
    pub seed_assets: Vec<(String, String)>,
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
            post_org_write_hook: None,
            seed_assets: Vec::new(),
        }
    }

    pub fn with_loro_storage(root_directory: PathBuf, loro_storage_dir: PathBuf) -> Self {
        // Canonicalize to resolve symlinks (e.g., /var -> /private/var on macOS)
        let root_directory = std::fs::canonicalize(&root_directory).unwrap_or(root_directory);
        let loro_storage_dir = std::fs::canonicalize(&loro_storage_dir).unwrap_or(loro_storage_dir);
        Self {
            root_directory,
            loro_storage_dir,
            post_org_write_hook: None,
            seed_assets: Vec::new(),
        }
    }
}

/// Seed `config.seed_assets` into an empty vault (no .org files) through the
/// `FileSystem` port. Must run before anything scans the vault (the initial
/// `OrgModeSyncProvider::sync` and the controller's initial scan). Panics on
/// write failure — a half-seeded vault on first launch is a startup error.
async fn seed_default_org_assets(fs: &dyn holon_filesystem::FileSystem, config: &OrgModeConfig) {
    if config.seed_assets.is_empty() {
        return;
    }
    let root = &config.root_directory;
    let scanned = crate::file_watcher::scan_directory(fs, root)
        .await
        .unwrap_or_else(|e| panic!("Failed to scan org root {}: {e}", root.display()));
    if !scanned.files.is_empty() {
        return;
    }
    fs.create_dir_all(root)
        .await
        .unwrap_or_else(|e| panic!("Failed to create org root {}: {e}", root.display()));
    for (filename, content) in &config.seed_assets {
        fs.write(&root.join(filename), content.as_bytes())
            .await
            .unwrap_or_else(|e| panic!("Failed to write {filename}: {e}"));
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

        // FileSystem port (ADR 0011): default-bind the real-disk adapter.
        // First binding wins in fluxdi, so a test harness that registered an
        // in-memory FileSystem before this module keeps its binding — that
        // case is expected and disclosed below, any other provide error is not.
        match injector.try_provide::<dyn holon_filesystem::FileSystem>(Provider::root(|_| {
            Arc::new(holon_filesystem::RealFileSystem) as Arc<dyn holon_filesystem::FileSystem>
        })) {
            Ok(()) => {}
            // Published fluxdi (dev @ 24b6eebb) models this as a struct
            // `Error { kind, .. }`, not an enum variant.
            Err(e) if matches!(e.kind, fluxdi::ErrorKind::ProviderAlreadyRegistered) => {
                info!(
                    "[OrgModeModule] dyn FileSystem already bound — keeping the \
                     existing (test-override) binding"
                );
            }
            Err(e) => return Err(e),
        }

        // FileChangeSource port (ADR 0011): default-bind the notify adapter.
        // Same first-binding-wins override contract as dyn FileSystem above.
        match injector.try_provide::<dyn holon_filesystem::FileChangeSource>(Provider::root(|_| {
            Arc::new(
                holon_filesystem::NotifyWatcher::new_unarmed()
                    .expect("notify watcher construction failed"),
            ) as Arc<dyn holon_filesystem::FileChangeSource>
        })) {
            Ok(()) => {}
            Err(e) if matches!(e.kind, fluxdi::ErrorKind::ProviderAlreadyRegistered) => {
                info!(
                    "[OrgModeModule] dyn FileChangeSource already bound — keeping \
                     the existing (test-override) binding"
                );
            }
            Err(e) => return Err(e),
        }

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
            let fs = resolver.resolve::<dyn holon_filesystem::FileSystem>();
            Shared::new(OrgModeSyncProvider::new(
                config.root_directory.clone(),
                token_store,
                fs,
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

                // Resolve remaining services
                let config = resolver.resolve::<OrgModeConfig>();

                // Seed default documents into an empty vault through the
                // FileSystem port BEFORE the initial sync task and the
                // controller's initial scan below — both consume the vault.
                {
                    let seed_fs = resolver.resolve::<dyn holon_filesystem::FileSystem>();
                    seed_default_org_assets(seed_fs.as_ref(), &config).await;
                }

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
                let db_handle_provider = resolver.resolve::<dyn holon::di::DbHandleProvider>();
                let db_handle = db_handle_provider.handle();

                // FileSyncController writes through SQL ops; CacheBlockReader reads from QueryableCache
                // which is also backed by the same Turso database, ensuring consistency.
                let sql_ops = Arc::new(holon::core::SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));

                let command_bus: Arc<dyn OriginTaggedWrites> =
                    sql_ops.clone() as Arc<dyn OriginTaggedWrites>;

                // ============================================================
                // PHASE 2: Create FileSyncController
                // Single controller using last_projection for echo suppression.
                // ============================================================
                info!("[OrgMode] Phase 2: Creating FileSyncController");

                info!("[OrgMode] Phase 2 complete");

                // ============================================================
                // PHASE 3: Spawn background tasks
                // The DatabaseActor serializes all operations, eliminating race conditions
                // between DDL and DML operations.
                // ============================================================
                info!("[OrgMode] Phase 3: Spawning background tasks");

                // Block writes go through FileSyncController → command_bus
                // (`SqlOperationProvider`) → the `block_raw` table directly; the
                // block cache and downstream sinks react via CDC / `LiveData<Block>`.

                // Option B: directory + file caches are fed DIRECTLY from the
                // `OrgModeSyncProvider` change broadcast — the EventBus middleman
                // (`OrgModeEventAdapter` → EventBus → `CacheEventSubscriber`) is
                // gone. We subscribe to the broadcast HERE, *before* the initial
                // sync task below runs, so the snapshot it emits is captured
                // gap-free. (The old EventBus replay/cursor only existed because the
                // adapter subscribed *after* sync — a fixable ordering bug, not an
                // inherent need for a durable replay buffer. Batches are coarse
                // `Vec<Change>` messages, so the broadcast buffer never lags.)
                {
                    let dir_cache = resolver.resolve_async::<QueryableCache<Directory>>().await;
                    let file_cache = resolver.resolve_async::<QueryableCache<File>>().await;
                    let mut dir_rx = sync_provider.subscribe_directories();
                    let mut file_rx = sync_provider.subscribe_files();
                    tokio::spawn(async move {
                        loop {
                            match dir_rx.recv().await {
                                Ok(batch) => {
                                    if let Err(e) = dir_cache.apply_batch(&batch.inner, None).await
                                    {
                                        error!("[directory cache feed] apply_batch failed: {}", e);
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!(
                                        "[directory cache feed] lagged by {} batches",
                                        n
                                    );
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                    tokio::spawn(async move {
                        loop {
                            match file_rx.recv().await {
                                Ok(batch) => {
                                    if let Err(e) = file_cache.apply_batch(&batch.inner, None).await
                                    {
                                        error!("[file cache feed] apply_batch failed: {}", e);
                                    }
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                    tracing::warn!("[file cache feed] lagged by {} batches", n);
                                }
                                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                            }
                        }
                    });
                }

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

                // FileSyncController: unified file ↔ block sync
                // Reacts to block changes via the shared `LiveData<Block>` feed
                // (works with both Loro and SQL paths). Runs on a single task via
                // tokio::select!, serializing on_file_changed and on_block_changed
                // — no locks needed.
                {
                    let command_bus = command_bus.clone();
                    let config_clone = config.clone();
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
                    let db_handle_for_live_docs = db_handle.clone();
                    let command_bus_for_docs = command_bus.clone();
                    // Phase 5 keystone: the convergent block feed for the shadow
                    // `wait_for_blocks_in_feed`. Present in both modes (EventInfraModule
                    // registers it); `None` only in configs without it.
                    let block_feed = resolver
                        .optional_resolve_async::<holon::sync::event_infra_module::BlockFeed>()
                        .await
                        .map(|bf| bf.0.clone());
                    let fs = resolver.resolve::<dyn holon_filesystem::FileSystem>();
                    let change_source =
                        resolver.resolve::<dyn holon_filesystem::FileChangeSource>();

                    tokio::spawn(async move {
                        use tracing::Instrument;
                        let doc_manager: Arc<dyn DocumentManager> = Arc::new(
                            async {
                                LiveDocumentManager::new(
                                    command_bus_for_docs,
                                    db_handle_for_live_docs,
                                )
                                .await
                                .expect("Failed to create LiveDocumentManager")
                            }
                            .instrument(tracing::info_span!("org.startup.live_doc_manager_new"))
                            .await,
                        );

                        let mut reader = CacheBlockReader::new(block_cache);
                        if let Some(feed) = &block_feed {
                            reader = reader.with_block_feed(feed.clone());
                        }
                        let block_reader: Arc<dyn BlockReader> = Arc::new(reader);

                        let mut controller = FileSyncController::new(
                            block_reader,
                            doc_manager,
                            config_clone.root_directory.clone(),
                            ordering,
                            fs.clone(),
                        );

                        if let Some(hook_cmd) = config_clone.post_org_write_hook.clone() {
                            controller = controller.with_post_org_write_hook(hook_cmd);
                        }

                        if let Some(downstream) = downstream {
                            controller = controller.with_downstream_projection(downstream);
                        }

                        if let Some(ref ops) = loro_ops_clone {
                            let shared_doc_store = ops.shared_doc_store();
                            let alias_registrar: Arc<
                                dyn crate::file_sync_controller::AliasRegistrar,
                            > = Arc::new(LoroAliasRegistrar {
                                doc_store: shared_doc_store,
                            });
                            controller = controller.with_alias_registrar(alias_registrar);
                        }

                        // Phase 5: drive org re-render from the convergent block feed
                        // (`LiveData<Block>`), replacing the EventBus `Consumer::ORG`
                        // block subscription. A resolver task consumes the feed's
                        // `MapDiff` stream, maps each changed block to its owning
                        // document (walk `parent_id` up to the nearest `Page`), and
                        // funnels re-render requests over a channel into the
                        // single-owner `select!` loop below (which alone holds
                        // `&mut controller`). Mirrors the Phase-4b link re-feed.
                        let (rerender_tx, rerender_rx) =
                            tokio::sync::mpsc::unbounded_channel::<OrgRerender>();
                        if let Some(feed) = block_feed.clone() {
                            let resolver_feed = feed.clone();
                            let tx = rerender_tx.clone();
                            tokio::spawn(async move {
                                use futures_signals::signal_map::{MapDiff, SignalMapExt};
                                resolver_feed
                                    .signal_map()
                                    .for_each(move |diff| {
                                        let tx = tx.clone();
                                        let feed = feed.clone();
                                        async move {
                                            let msg = match diff {
                                                MapDiff::Insert { value, .. }
                                                | MapDiff::Update { value, .. } => {
                                                    match resolve_doc_for_block(&feed, &value) {
                                                        Some(doc) => OrgRerender::Doc(doc),
                                                        // ALLOW(fallback): doc unresolved (matview lag / nested) → full re-render
                                                        None => OrgRerender::All,
                                                    }
                                                }
                                                MapDiff::Remove { .. }
                                                | MapDiff::Replace { .. }
                                                | MapDiff::Clear {} => OrgRerender::All,
                                            };
                                            let _ = tx.send(msg);
                                        }
                                    })
                                    .await;
                            });
                        }

                        run_file_sync_controller(
                            controller,
                            config_clone.root_directory.clone(),
                            idle_signal_weak,
                            rerender_rx,
                            ready_sender_clone,
                            fs,
                            change_source,
                        )
                        .await;
                    });
                }

                // Block command-path provider for the UI dispatcher.
                //
                // Under Loro authority (Loro enabled) block CRUD
                // (set_field / create / update / delete) must land in the
                // Loro doc — the source of truth — so `LoroSyncController`
                // can project it to the SQL `block_raw` table + matview.
                // Routing these through the generic `SqlOperationProvider`
                // (SQL-direct) instead made content edits on non-rendered
                // blocks drift: SQL got the new value, Loro never did
                // (see `tests/loro_content_drop_pbt.rs`). This realizes the
                // behaviour this factory's header already promised
                // ("resolve LoroBlockOperations if available").
                //
                // In SqlOnly mode there is no Loro, so SQL is the authority
                // and writes go straight to `block_raw`.
                //
                // Structural ops (indent / split / move) are served by the
                // earlier-registered `SqlBlockOperations` (EventInfraModule),
                // which wins them on registration order; this provider only
                // wins the CRUD ops that `SqlBlockOperations` does not
                // advertise.
                match loro_ops {
                    Some(loro_ops) => {
                        let wrapper = OperationWrapper::new(loro_ops, Some(sync_provider));
                        Arc::new(wrapper) as Arc<dyn OperationProvider>
                    }
                    None => {
                        let wrapper = OperationWrapper::new(sql_ops.clone(), Some(sync_provider));
                        Arc::new(wrapper) as Arc<dyn OperationProvider>
                    }
                }
            }
        }));

        Ok(())
    }
}

/// A re-render request funnelled from the block-feed resolver task into the
/// org controller's single-owner `select!` loop (Phase 5: replaces the EventBus
/// `Consumer::ORG` block-event path).
pub enum OrgRerender {
    /// Re-render exactly this document (resolved to its `Page` root).
    Doc(EntityUri),
    /// Document could not be resolved (matview lag, deleted block, etc.) —
    /// fall back to a debounced re-render of every tracked file.
    All,
}

/// Resolve the owning document URI for a feed `Block` by walking `parent_id`
/// up the in-memory block feed to the nearest `Page`-tagged ancestor (the block
/// itself included). Mirrors `SqlOperationProvider::find_document_uri`'s
/// recursive CTE (depth-bounded at 50). Returns `None` when the chain ends
/// without a `Page` — e.g. an ancestor not yet present in the matview-backed
/// feed — and the caller falls back to a full re-render.
fn resolve_doc_for_block(feed: &holon::sync::LiveData<Block>, block: &Block) -> Option<EntityUri> {
    let map = feed.read();
    let mut current = block.clone();
    for _ in 0..50 {
        if current.is_page() {
            return Some(current.id.clone());
        }
        match map.get(current.parent_id.as_str()) {
            Some(parent) => current = (**parent).clone(),
            None => return None,
        }
    }
    None
}

/// Generic boot-time file-sync spawn: build a [`FileSyncController`] over the given domain
/// seams (`BlockReader` / `DocumentManager` / `BlockOrdering`) and `FileFormatAdapter`, then
/// `tokio::spawn` [`run_file_sync_controller`]. Backend- AND format-agnostic per ADR 0004:
/// the caller injects the seam adapters for whichever store is canonical and the format
/// adapter for whichever on-disk serialization (org / markdown / …) — there is no
/// adapter-pair-named bootstrap. The returned strong `Arc<OrgSyncIdleSignal>` keeps the loop
/// alive (the loop holds a `Weak` and exits when the caller drops it). Call from an async
/// tokio context (so `tokio::spawn` has a runtime).
///
/// Block→file re-render is the caller's concern and stays dormant here (a closed rerender
/// channel) — matching the no-Turso bootstrap.
#[allow(clippy::too_many_arguments)]
pub fn spawn_file_sync(
    root_directory: PathBuf,
    block_reader: Arc<dyn BlockReader>,
    doc_manager: Arc<dyn DocumentManager>,
    ordering: Arc<dyn BlockOrdering>,
    format: Arc<dyn holon_core::FileFormatAdapter>,
    alias_registrar: Option<Arc<dyn crate::file_sync_controller::AliasRegistrar>>,
    fs: Arc<dyn holon_filesystem::FileSystem>,
    change_source: Arc<dyn holon_filesystem::FileChangeSource>,
) -> (
    tokio::sync::watch::Receiver<Option<Result<(), String>>>,
    Arc<OrgSyncIdleSignal>,
) {
    // Canonicalize the root the way `OrgModeConfig::new` does so `scan_org_files` and
    // `on_file_changed` strip_prefix against the canonical form (macOS /tmp → /private/tmp).
    let root_directory = fs.canonicalize(&root_directory).unwrap_or(root_directory);
    let mut controller = FileSyncController::with_format(
        block_reader,
        doc_manager,
        root_directory.clone(),
        format,
        ordering,
        fs.clone(),
    );
    if let Some(registrar) = alias_registrar {
        controller = controller.with_alias_registrar(registrar);
    }

    let (ready_sender, ready_signal) = FileWatcherReadySignal::new();
    let ready_sender_arc = Arc::new(std::sync::Mutex::new(Some(ready_sender)));

    let idle = OrgSyncIdleSignal::new();
    let idle_weak = Arc::downgrade(&idle);

    // Dormant rerender channel — dropping `_tx` keeps the `rerender_rx` select arm idle.
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

/// Backend-blind FileSyncController driver: initialize, build the file watcher,
/// run the initial scan, signal readiness, arm the watcher, and run the main
/// `select!` loop. Shared by the Turso factory and the no-Turso bootstrap —
/// neither path knows which storage backend the controller's adapters use.
pub async fn run_file_sync_controller(
    mut controller: FileSyncController,
    root_directory: PathBuf,
    idle_signal_weak: std::sync::Weak<OrgSyncIdleSignal>,
    mut rerender_rx: tokio::sync::mpsc::UnboundedReceiver<OrgRerender>,
    ready_sender: std::sync::Arc<std::sync::Mutex<Option<FileWatcherReadySender>>>,
    fs: Arc<dyn holon_filesystem::FileSystem>,
    change_source: Arc<dyn holon_filesystem::FileChangeSource>,
) {
    use tracing::{error, info, Instrument};

    let init_result = async { controller.initialize().await }
        .instrument(tracing::info_span!("org.startup.controller_initialize"))
        .await;
    if let Err(e) = init_result {
        let msg = format!("FileSyncController initialization failed: {}", e);
        error!("[OrgMode] {}", msg);
        if let Some(sender) = ready_sender.lock().unwrap().take() {
            sender.signal_error(msg);
        }
        return;
    }

    // Build the org filter bridge over the change-source port without arming
    // it yet — the slow recursive watch registration (9+s on macOS for the
    // notify adapter) is deferred until after signal_ready so the factory can
    // return immediately. The bridge subscribes here, so no event is missed.
    let mut file_rx = tracing::info_span!("org.startup.file_watcher_new_unarmed")
        .in_scope(|| OrgFileWatcher::new(change_source.as_ref(), &root_directory))
        .into_receiver();
    info!(
        "[OrgMode] File watcher built (unarmed) for: {}",
        root_directory.display()
    );

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
        let org_files = match scan_org_files(fs.as_ref(), &root_directory).await {
            Ok(files) => files,
            Err(e) => {
                return vec![(
                    root_directory.clone(),
                    anyhow::Error::from(e)
                        .context(format!("initial scan of {}", root_directory.display())),
                )];
            }
        };
        let fs_warm = fs.clone();
        let preloaded: Vec<(std::path::PathBuf, Option<String>)> =
            futures::future::join_all(org_files.into_iter().map(|p| {
                let fs_warm = fs_warm.clone();
                async move {
                    let content = fs_warm.read_to_string(&p).await.ok(); // ALLOW(ok): best-effort OS page-cache warmup; content is dropped below
                    (p, content)
                }
            }))
            .instrument(tracing::info_span!("org.initial_scan.parallel_read"))
            .await;
        let mut failures = Vec::new();
        for (file_path, _content) in preloaded {
            if let Err(e) = controller.on_file_changed(&file_path).await {
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
        if let Some(sender) = ready_sender.lock().unwrap().take() {
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
    // `scan_directory` (see file_sync_controller.rs).
    // Without that Phase A→B extension, this fix
    // breaks `create_document`.
    if let Some(sender) = ready_sender.lock().unwrap().take() {
        sender.signal_ready();
    }

    // Spawn arm() on the blocking pool, detached.
    // Holds a strong ref to the change source alive
    // forever via `pending::<()>().await` — dropping
    // it (e.g. the notify adapter's RecommendedWatcher)
    // silently stops event delivery into `file_rx`.
    // AbortOnDrop wraps the JoinHandle so this task
    // terminates when the outer file-watcher loop
    // exits via the Weak<OrgSyncIdleSignal> shutdown.
    let dir_for_arm = root_directory.clone();
    let source_for_arm = change_source.clone();
    let arm_task = tokio::spawn(
        async move {
            let r = tokio::task::spawn_blocking(move || {
                let r = source_for_arm.arm(&dir_for_arm);
                (source_for_arm, r)
            })
            .await;
            match r {
                Ok((source, Ok(()))) => {
                    info!("[OrgMode] watcher armed");
                    let _kept = source;
                    std::future::pending::<()>().await;
                }
                Ok((_, Err(e))) => {
                    error!("[OrgMode] watch_recursive failed: {}", e);
                }
                Err(e) => {
                    error!("[OrgMode] arm spawn_blocking panicked: {}", e);
                }
            }
        }
        .instrument(tracing::info_span!("org.startup.arm_watcher_blocking")),
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
    let mut poll_tick = tokio::time::interval(tokio::time::Duration::from_millis(100));
    poll_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut discovery_tick = tokio::time::interval(tokio::time::Duration::from_secs(2));
    discovery_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // Coalesce mark_processed across bursty events; flushes
    // Coalesce orphan-event full re-renders. Events that
    // lack routing_doc_uri (and whose payload parent_id
    // doesn't resolve via on_block_changed) used to trigger
    // re_render_all_tracked per event — O(events × tracked
    // files) IO + segment-chain lookups during bursty
    // initial scans. The flag is set in the event arm; a
    // 50ms ticker drains it with a single re-render pass.
    let mut pending_full_rerender = false;
    let mut rerender_flush_tick = tokio::time::interval(tokio::time::Duration::from_millis(50));
    rerender_flush_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        // Session-alive check: if the strong refs to
        // OrgSyncIdleSignal have all been dropped, the
        // owning FrontendSession is gone — exit.
        let Some(idle_signal_for_task) = idle_signal_weak.upgrade() else {
            info!("[OrgMode] file-watcher loop exiting (session dropped)");
            return;
        };
        tokio::select! {
            Some((maybe_path, change_seq)) = file_rx.recv() => {
                if let Some(file_path) = maybe_path {
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
                // Advance even on error / filtered events: the change was
                // handled (errors are surfaced above); a wedged watermark
                // would turn one logged failure into every later
                // wait_for_change_seq timing out.
                idle_signal_for_task.advance_change_seq(change_seq);
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
            Some(rerender) = rerender_rx.recv() => {
                let span = tracing::info_span!("org.on_block_feed");
                async {
                    match rerender {
                        OrgRerender::Doc(doc_id) => {
                            match controller.on_block_changed(&doc_id).await {
                                Ok(true) => {}
                                // ALLOW(fallback): doc resolved to no tracked file → full re-render
                                Ok(false) => { pending_full_rerender = true; }
                                Err(e) => {
                                    error!(
                                        "[OrgMode] Block change error for {}: {}",
                                        doc_id, e
                                    );
                                }
                            }
                        }
                        OrgRerender::All => { pending_full_rerender = true; }
                    }
                }.instrument(span).await;
                idle_signal_for_task.mark_progress();
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

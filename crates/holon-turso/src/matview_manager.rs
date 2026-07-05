//! Manages materialized view lifecycle — creation, existence checks,
//! orphan cleanup, CDC subscription, and querying.
//!
//! Consolidates the matview lifecycle that was previously duplicated across
//! `BackendEngine::preload_views`, `BackendEngine::watch_query`, and `WatchedQuery::new`.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{Context, Result};
use async_trait::async_trait;
use tokio::sync::{broadcast, mpsc, oneshot};

use crate::sql_parser::{extract_table_refs, parse_sql};
use crate::turso::priority;
use crate::turso::{DbHandle, RowChange, RowChangeStream};
use crate::util::strip_order_by;
use holon_api::{BatchWithMetadata, Value};
use holon_core::storage::{Resource, StorageEntity};

/// Normalize a SQL statement for comparison: collapse whitespace, strip trailing
/// semicolons, lowercase keywords, and drop spaces before `(` (Turso's view
/// pretty-printer emits `iif (` / `strftime (`). This lets us compare
/// `sqlite_master.sql` against the desired CREATE statement without false
/// positives from formatting differences.
fn normalize_view_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(';')
        .to_lowercase()
        .replace(" (", "(")
}

/// Reconcile a named materialized view: only DROP+CREATE if the SELECT changed.
///
/// Accepts just the SELECT query (like `ensure_view` does for dynamic views) and
/// constructs the full `CREATE MATERIALIZED VIEW {name} AS {select}` itself.
/// Compares against `sqlite_master.sql` to detect changes.
///
/// This is a free function taking `DbHandle` so it can be called from `SchemaModule`
/// implementations that don't have access to `MatviewManager`.
///
/// Returns `true` if the view was (re)created, `false` if it already matched.
pub async fn reconcile_named_view(
    db_handle: &DbHandle,
    view_name: &str,
    select_sql: &str,
) -> Result<bool> {
    let create_sql = format!("CREATE MATERIALIZED VIEW {} AS {}", view_name, select_sql);

    let rows = db_handle
        .query(
            &format!(
                "SELECT sql FROM sqlite_master WHERE type='view' AND name='{}'",
                view_name
            ),
            HashMap::new(),
        )
        .await?;

    if let Some(row) = rows.first() {
        if let Some(Value::String(existing_sql)) = row.get("sql") {
            if normalize_view_sql(existing_sql) == normalize_view_sql(&create_sql) {
                tracing::debug!(
                    "[reconcile_named_view] View '{}' unchanged, skipping",
                    view_name
                );
                return Ok(false);
            }
            tracing::info!(
                "[reconcile_named_view] View '{}' definition changed, recreating",
                view_name
            );
        }
        db_handle
            .execute_ddl(&format!("DROP VIEW IF EXISTS {}", view_name))
            .await?;
    }

    // Idempotent-or-recovering create. The `sqlite_master` probe above only sees
    // rows of `type='view'`; a crash mid-DDL can leave the matview's backing
    // table and/or its `__turso_internal_dbsp_state_*` tables behind WITHOUT a
    // committed view row. A plain `CREATE MATERIALIZED VIEW` then dies with
    // "table <name> already exists" and the app boot-loops (on Android the only
    // escape was `pm clear` = user data loss). Recover instead: clean the
    // orphaned DBSP state, `CREATE ... IF NOT EXISTS`, and on a residual
    // collision drop the orphaned backing objects and retry ONCE. This is
    // data-safe: a matview is a pure projection over its base tables (e.g.
    // `block` over `block_raw`), which are never touched here, so the source of
    // truth survives. If the retry still fails we surface a clear error rather
    // than looping forever.
    cleanup_orphaned_dbsp_state(db_handle, view_name).await?;
    let create_idempotent = format!(
        "CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS {}",
        view_name, select_sql
    );
    match db_handle.execute_ddl(&create_idempotent).await {
        Ok(()) => {
            tracing::info!(
                "[reconcile_named_view] View '{}' created/updated",
                view_name
            );
            Ok(true)
        }
        Err(e) if e.to_string().contains("already exists") => {
            tracing::warn!(
                "[reconcile_named_view] View '{view_name}' collided with orphaned \
                 backing objects left by a prior crash ({e}); dropping the derived \
                 matview + orphaned DBSP state and recreating (base tables are \
                 untouched — no data loss)"
            );
            db_handle
                .execute_ddl(&format!("DROP VIEW IF EXISTS {}", view_name))
                .await?;
            db_handle
                .execute_ddl(&format!("DROP TABLE IF EXISTS {}", view_name))
                .await?;
            cleanup_orphaned_dbsp_state(db_handle, view_name).await?;
            db_handle
                .execute_ddl(&create_idempotent)
                .await
                .map_err(|e2| {
                    anyhow::anyhow!(
                        "reconcile_named_view: matview '{view_name}' could not be \
                         (re)created even after clearing orphaned backing objects \
                         left by a prior crash: {e2}. The schema is genuinely \
                         incompatible; failing loudly instead of boot-looping. Base \
                         tables are intact — inspect the DB rather than clearing \
                         user data."
                    )
                })?;
            tracing::info!(
                "[reconcile_named_view] View '{}' recovered and recreated",
                view_name
            );
            Ok(true)
        }
        Err(e) => Err(e.into()),
    }
}

/// Drop the `__turso_internal_dbsp_state_v*_{view_name}` tables left orphaned by
/// a crash mid-DDL. These are Turso IVM internal state, NOT user data — the
/// matview is rebuilt from its base tables on recreate. Mirrors
/// [`MatviewManager::cleanup_orphaned_dbsp_tables`] but as a free function so
/// the boot-time [`reconcile_named_view`] (which has no `MatviewManager`) can
/// share the recovery path.
async fn cleanup_orphaned_dbsp_state(db_handle: &DbHandle, view_name: &str) -> Result<()> {
    let pattern = format!("__turso_internal_dbsp_state_v%_{}", view_name);
    let check_sql = format!(
        "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '{}'",
        pattern
    );
    let orphaned = db_handle.query(&check_sql, HashMap::new()).await?;
    for row in orphaned {
        if let Some(Value::String(table_name)) = row.get("name") {
            tracing::debug!(
                "[reconcile_named_view] Cleaning orphaned DBSP state table: {}",
                table_name
            );
            db_handle
                .execute_ddl(&format!("DROP TABLE IF EXISTS {}", table_name))
                .await?;
        }
    }
    Ok(())
}

// MatviewHook (the FDW-primed callback) is a storage-agnostic trait; it lives
// in holon-core so providers implement it without naming the Turso backend.
use holon_core::MatviewHook;

/// Result of watching a query — initial data + CDC stream.
pub struct WatchResult {
    pub initial_rows: Vec<StorageEntity>,
    pub stream: RowChangeStream,
    pub view_name: String,
}

/// Command sent to the CDC demultiplexer task.
enum DemuxCommand {
    /// Register a new subscriber for a specific view. `ack` fires once the
    /// subscriber is registered, so callers can order registration before
    /// their initial query.
    Subscribe {
        view_name: String,
        tx: mpsc::Sender<BatchWithMetadata<RowChange>>,
        ack: oneshot::Sender<()>,
    },
}

/// Manages the full lifecycle of Turso materialized views.
///
/// CDC routing uses a single demultiplexer task instead of spawning one filter
/// task per `subscribe_cdc()` call. The demux task reads from the broadcast
/// channel and routes batches to registered subscribers by `relation_name`.
/// Closed subscribers are pruned automatically.
/// @c4 code
pub struct MatviewManager {
    db_handle: DbHandle,
    demux_cmd_tx: mpsc::Sender<DemuxCommand>,
    ddl_mutex: Arc<tokio::sync::Mutex<()>>,
    /// Cache tables that have an associated FDW table (`{name}_fdw`).
    fdw_backed_tables: Arc<tokio::sync::RwLock<HashSet<String>>>,
    /// Optional hook called after FDW cache priming.
    hook: Arc<tokio::sync::RwLock<Option<Arc<dyn MatviewHook>>>>,
    /// In-memory cache of view names known to exist in `sqlite_master`.
    /// Populated by `ensure_view`/`preload` after the first existence check or
    /// successful CREATE, so subsequent calls skip the SQL round trip. Cleared
    /// by `drop_stale_views`. Process-local; views are deterministic hashes of
    /// their SELECT SQL so re-issuing CREATE under contention is safe (`IF NOT
    /// EXISTS`), but each redundant existence query was 5-15 ms and they
    /// dominated PBT `check_invariants` overhead.
    known_views: Arc<tokio::sync::RwLock<HashSet<String>>>,
    /// Counters for measuring cache effectiveness. `cache_hits` is the number
    /// of `ensure_view`/`preload` calls that returned via the in-memory cache
    /// without a `view_exists` SQL round trip. `exists_calls` is the number of
    /// `view_exists` SQL round trips actually issued. `ddl_creates` counts
    /// successful CREATE MATERIALIZED VIEW executions.
    cache_hits: Arc<AtomicU64>,
    exists_calls: Arc<AtomicU64>,
    ddl_creates: Arc<AtomicU64>,
}

impl MatviewManager {
    pub fn new(db_handle: DbHandle, ddl_mutex: Arc<tokio::sync::Mutex<()>>) -> Self {
        let demux_cmd_tx = Self::spawn_demux(db_handle.cdc_broadcast().clone());
        Self {
            db_handle,
            demux_cmd_tx,
            ddl_mutex,
            fdw_backed_tables: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            hook: Arc::new(tokio::sync::RwLock::new(None)),
            known_views: Arc::new(tokio::sync::RwLock::new(HashSet::new())),
            cache_hits: Arc::new(AtomicU64::new(0)),
            exists_calls: Arc::new(AtomicU64::new(0)),
            ddl_creates: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Snapshot of (cache_hits, exists_calls, ddl_creates). Useful for tests
    /// and one-off profiling — the cache is a hot path so we keep counters
    /// in atomics even in release builds.
    pub fn cache_metrics(&self) -> (u64, u64, u64) {
        (
            self.cache_hits.load(Ordering::Relaxed),
            self.exists_calls.load(Ordering::Relaxed),
            self.ddl_creates.load(Ordering::Relaxed),
        )
    }

    /// Register a cache table as FDW-backed. Matview creation will auto-prime
    /// the cache from the FDW before building the view.
    pub async fn register_fdw_table(&self, cache_table: &str) {
        self.fdw_backed_tables
            .write()
            .await
            .insert(cache_table.to_string());
    }

    /// Set the hook called after successful FDW cache priming.
    pub async fn set_hook(&self, hook: Arc<dyn MatviewHook>) {
        *self.hook.write().await = Some(hook);
    }

    /// Spawn the single CDC demultiplexer task.
    ///
    /// Reads from the broadcast channel and fans out to per-view subscribers.
    /// Dead subscribers (closed channels) are pruned on each batch.
    fn spawn_demux(
        cdc_broadcast: broadcast::Sender<BatchWithMetadata<RowChange>>,
    ) -> mpsc::Sender<DemuxCommand> {
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<DemuxCommand>(64);
        let mut broadcast_rx = cdc_broadcast.subscribe();
        crate::util::spawn_actor(async move {
            let mut subscribers: HashMap<String, Vec<mpsc::Sender<BatchWithMetadata<RowChange>>>> =
                HashMap::new();
            let mut cmd_rx_open = true;

            loop {
                // Stop when no subscribers remain AND the command channel is closed
                // (no new subscribers can arrive)
                if !cmd_rx_open && subscribers.is_empty() {
                    break;
                }

                tokio::select! {
                    // Process new subscriber registrations (only when channel is open)
                    maybe_cmd = cmd_rx.recv(), if cmd_rx_open => {
                        match maybe_cmd {
                            Some(DemuxCommand::Subscribe { view_name, tx, ack }) => {
                                tracing::info!("[Demux] Registered subscriber for '{}'", view_name);
                                subscribers.entry(view_name).or_default().push(tx);
                                // Receiver gone = caller aborted before registration
                                // completed; nothing to notify.
                                let _ = ack.send(());
                            }
                            None => {
                                // MatviewManager dropped — stop accepting new subscribers
                                // but keep delivering to existing ones
                                cmd_rx_open = false;
                            }
                        }
                    }
                    // Route CDC batches to matching subscribers
                    result = broadcast_rx.recv() => {
                        match result {
                            Ok(batch) => {
                                let view_name = &batch.metadata.relation_name;
                                let sub_count = subscribers.get(view_name).map(|s| s.len()).unwrap_or(0);
                                if !batch.inner.items.is_empty() {
                                    if sub_count > 0 {
                                        tracing::info!(
                                            "[Demux] view='{}' items={} subscribers={}",
                                            view_name, batch.inner.items.len(), sub_count
                                        );
                                    } else {
                                        tracing::trace!(
                                            "[Demux] view='{}' items={} subscribers=0",
                                            view_name, batch.inner.items.len()
                                        );
                                    }
                                }
                                if let Some(senders) = subscribers.get_mut(view_name) {
                                    senders.retain(|tx| {
                                        match tx.try_send(batch.clone()) {
                                            Ok(()) => true,
                                            Err(mpsc::error::TrySendError::Full(_)) => {
                                                // A dropped delta would silently corrupt the
                                                // subscriber's incremental state forever (lost
                                                // rows / ghost rows). Close the stream instead so
                                                // the consumer sees the end-of-stream and must
                                                // resubscribe via watch(), re-querying initial rows.
                                                tracing::error!(
                                                    "[MatviewManager] CDC subscriber for '{}' is full; \
                                                     closing its stream (delivering a partial delta \
                                                     stream would corrupt incremental consumers)",
                                                    view_name
                                                );
                                                false
                                            }
                                            Err(mpsc::error::TrySendError::Closed(_)) => false,
                                        }
                                    });
                                    if senders.is_empty() {
                                        subscribers.remove(view_name);
                                    }
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(
                                    "[MatviewManager] CDC demux lagged by {} messages",
                                    n
                                );
                            }
                            Err(broadcast::error::RecvError::Closed) => {
                                break;
                            }
                        }
                    }
                }
            }
        });

        cmd_tx
    }

    /// Drop all `watch_view_*` materialized views left over from a previous session.
    ///
    /// Turso IVM matviews can become stale across app restarts (e.g., when document
    /// UUIDs change or the underlying data is re-synced). Dropping them ensures they
    /// get recreated fresh with correct IVM state.
    pub async fn drop_stale_views(&self) -> Result<()> {
        let rows = self
            .db_handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='view' AND name LIKE 'watch_view_%'",
                HashMap::new(),
            )
            .await?;

        for row in &rows {
            if let Some(Value::String(name)) = row.get("name") {
                tracing::info!("[MatviewManager] Dropping stale view: {}", name);
                let drop_sql = format!("DROP VIEW IF EXISTS {}", name);
                self.db_handle.execute_ddl(&drop_sql).await?;
                self.cleanup_orphaned_dbsp_tables(name).await?;
            }
        }

        // Reset the in-memory cache: every view tracked there is either one we
        // just dropped or one that was never registered to begin with.
        self.known_views.write().await.clear();

        if !rows.is_empty() {
            tracing::info!("[MatviewManager] Dropped {} stale watch views", rows.len());
        }

        Ok(())
    }

    /// Hash SQL text into a deterministic view name.
    pub fn compute_view_name(sql: &str) -> String {
        let mut hasher = DefaultHasher::new();
        sql.hash(&mut hasher);
        format!("watch_view_{:x}", hasher.finish())
    }

    /// Ensure a materialized view exists for the given SQL, creating it if needed.
    ///
    /// Steps: prime FDW cache (if applicable) → check existence → acquire DDL mutex →
    /// double-check → clean orphaned DBSP state tables → strip ORDER BY →
    /// CREATE MATERIALIZED VIEW with dependency tracking.
    #[tracing::instrument(skip(self, sql), fields(view_name = tracing::field::Empty))]
    pub async fn ensure_view(&self, sql: &str) -> Result<String> {
        self.prime_fdw_caches(sql).await;

        let view_name = Self::compute_view_name(sql);
        tracing::Span::current().record("view_name", view_name.as_str());

        if self.is_view_known(&view_name).await {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                "[MatviewManager] View {} cached as known, reusing",
                view_name
            );
            return Ok(view_name);
        }

        if self.view_exists(&view_name).await {
            self.mark_view_known(&view_name).await;
            tracing::debug!(
                "[MatviewManager] View {} already exists, reusing",
                view_name
            );
            return Ok(view_name);
        }

        tracing::debug!(
            "[MatviewManager] View {} does not exist, creating...",
            view_name
        );

        let _ddl_guard = self.ddl_mutex.lock().await;
        tracing::debug!(
            "[MatviewManager] Acquired DDL mutex for view: {}",
            view_name
        );

        // Re-check the cache and sqlite_master under the DDL mutex — another
        // task may have created the view while we were waiting.
        if self.is_view_known(&view_name).await {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                "[MatviewManager] View {} cached while waiting for DDL mutex, reusing",
                view_name
            );
            return Ok(view_name);
        }
        if self.view_exists(&view_name).await {
            self.mark_view_known(&view_name).await;
            tracing::debug!(
                "[MatviewManager] View {} was created while waiting for DDL mutex, reusing",
                view_name
            );
            return Ok(view_name);
        }

        self.cleanup_orphaned_dbsp_tables(&view_name).await?;

        let sql_for_view = strip_order_by(sql);
        let create_view_sql = format!(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS {}",
            view_name, sql_for_view
        );
        tracing::debug!(
            "[MatviewManager] Creating materialized view: {}",
            create_view_sql
        );

        let provides = vec![Resource::schema(view_name.clone())];
        let requires = parse_sql(&sql_for_view)
            .map(|stmts| extract_table_refs(&stmts))
            .unwrap_or_default();

        tracing::debug!(
            "[MatviewManager] DDL deps — provides: {:?}, requires: {:?}",
            provides,
            requires
        );

        self.db_handle
            .execute_ddl_with_deps(&create_view_sql, provides, requires, priority::DDL_MATVIEW)
            .await
            .context("Failed to create materialized view")?;

        self.ddl_creates.fetch_add(1, Ordering::Relaxed);
        self.mark_view_known(&view_name).await;
        tracing::debug!("[MatviewManager] Successfully created view: {}", view_name);
        Ok(view_name)
    }

    /// Like `ensure_view` but retries on transient errors (for startup preloading).
    ///
    /// Logs warnings instead of failing — a preload failure is non-fatal because
    /// `watch_query` will create the view lazily later.
    pub async fn preload(&self, sql: &str) -> Result<String> {
        let view_name = Self::compute_view_name(sql);

        if self.is_view_known(&view_name).await {
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            tracing::debug!(
                "[MatviewManager] preload: view {} cached as known, skipping",
                view_name
            );
            return Ok(view_name);
        }

        if self.view_exists(&view_name).await {
            self.mark_view_known(&view_name).await;
            tracing::debug!(
                "[MatviewManager] preload: view {} already exists, skipping",
                view_name
            );
            return Ok(view_name);
        }

        let sql_for_view = strip_order_by(sql);
        let create_view_sql = format!(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS {} AS {}",
            view_name, sql_for_view
        );

        let mut last_error = None;
        for attempt in 0..3 {
            match self.db_handle.execute_ddl(&create_view_sql).await {
                Ok(_) => {
                    self.ddl_creates.fetch_add(1, Ordering::Relaxed);
                    self.mark_view_known(&view_name).await;
                    tracing::info!("[MatviewManager] preload: created view {}", view_name);
                    return Ok(view_name);
                }
                Err(e) => {
                    let err_str = format!("{:?}", e);
                    let is_retryable = err_str.contains("database is locked")
                        || err_str.contains("Database schema changed");
                    if is_retryable && attempt < 2 {
                        tracing::debug!(
                            "[MatviewManager] preload: retry {} for view {}: {}",
                            attempt + 1,
                            view_name,
                            err_str
                        );
                        tokio::time::sleep(std::time::Duration::from_millis(50 * (1 << attempt)))
                            .await;
                        last_error = Some(e);
                    } else {
                        last_error = Some(e);
                        break;
                    }
                }
            }
        }
        if let Some(e) = last_error {
            tracing::warn!(
                "[MatviewManager] preload: failed to create view {}: {}\n{}",
                view_name,
                e,
                create_view_sql
            );
        }
        Ok(view_name)
    }

    /// Query all rows from a materialized view.
    ///
    /// Includes Turso's internal `rowid` aliased as `_rowid` so that
    /// `LiveData` can build its `rowid → user-key` map for matview rows
    /// without an `id` column. Matches the shape `process_cdc_event`
    /// produces for live CDC events, where `_rowid` is injected into the
    /// `data` HashMap. Without this alignment, an initial row whose first
    /// post-load CDC event is a `Delete` (e.g. a `focus_roots` row whose
    /// region cursor is set to NULL via `NavigateBack` before any
    /// intermediate update) would never be removed from the LiveData.
    #[tracing::instrument(skip(self))]
    pub async fn query_view(&self, view_name: &str) -> Result<Vec<StorageEntity>> {
        let select_sql = format!("SELECT *, rowid AS _rowid FROM {}", view_name);
        self.db_handle
            .query(&select_sql, HashMap::new())
            .await
            .with_context(|| format!("Failed to query view {view_name}"))
    }

    /// Subscribe to CDC for a specific view, returning a filtered stream.
    ///
    /// Registers with the single demultiplexer task instead of spawning a
    /// per-subscription filter task. The demux routes batches by `relation_name`
    /// and prunes closed subscribers automatically. Awaits the demux's
    /// registration ack before returning, so a subsequent query is guaranteed
    /// to observe registration-before-query ordering.
    pub async fn subscribe_cdc(&self, view_name: &str) -> Result<RowChangeStream> {
        let (tx, rx) = mpsc::channel(1024);
        let (ack_tx, ack_rx) = oneshot::channel();
        tracing::info!("[MatviewManager] subscribe_cdc('{}')", view_name);
        self.demux_cmd_tx
            .send(DemuxCommand::Subscribe {
                view_name: view_name.to_string(),
                tx,
                ack: ack_tx,
            })
            .await
            .map_err(|e| {
                anyhow::anyhow!("failed to register CDC subscriber for '{view_name}': {e}")
            })?;
        ack_rx
            .await
            .map_err(|_| anyhow::anyhow!("CDC demux dropped subscription ack for '{view_name}'"))?;
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    /// Ensure a materialized view exists, query its initial data, and subscribe to CDC.
    #[tracing::instrument(skip(self, sql))]
    pub async fn watch(&self, sql: &str) -> Result<WatchResult> {
        let view_name = self.ensure_view(sql).await?;
        let stream = self.subscribe_cdc(&view_name).await?;
        let initial_rows = self.query_view(&view_name).await?;
        Ok(WatchResult {
            initial_rows,
            stream,
            view_name,
        })
    }

    async fn view_exists(&self, view_name: &str) -> bool {
        self.exists_calls.fetch_add(1, Ordering::Relaxed);
        let check_sql = format!(
            "SELECT name FROM sqlite_master WHERE type='view' AND name='{}'",
            view_name
        );
        match self.db_handle.query(&check_sql, HashMap::new()).await {
            Ok(results) => !results.is_empty(),
            Err(_) => false,
        }
    }

    async fn is_view_known(&self, view_name: &str) -> bool {
        self.known_views.read().await.contains(view_name)
    }

    async fn mark_view_known(&self, view_name: &str) {
        self.known_views.write().await.insert(view_name.to_string());
    }

    /// Prime FDW-backed cache tables referenced in the SQL.
    ///
    /// For each table in the SQL that has an FDW counterpart (`{table}_fdw`),
    /// rewrite the SQL to query the FDW table. This triggers the FDW's
    /// write-through, populating the cache table. Then calls the hook.
    async fn prime_fdw_caches(&self, sql: &str) {
        let fdw_tables = self.fdw_backed_tables.read().await;
        if fdw_tables.is_empty() {
            return;
        }

        let table_refs = parse_sql(sql)
            .map(|stmts| extract_table_refs(&stmts))
            .unwrap_or_default();

        for resource in &table_refs {
            let table_name = resource.name();
            if fdw_tables.contains(table_name) {
                let fdw_sql = sql.replace(table_name, &format!("{table_name}_fdw"));
                tracing::info!(
                    "[MatviewManager] Priming FDW cache for '{}': {}",
                    table_name,
                    &fdw_sql[..fdw_sql.len().min(200)]
                );
                match self.db_handle.query(&fdw_sql, HashMap::new()).await {
                    Ok(rows) => {
                        tracing::info!(
                            "[MatviewManager] FDW prime: {} rows written through to '{}'",
                            rows.len(),
                            table_name,
                        );
                        // Notify hook (e.g. subscribe to resource notifications)
                        if let Some(hook) = self.hook.read().await.as_ref() {
                            hook.on_fdw_primed(table_name, &fdw_sql).await;
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "[MatviewManager] FDW prime failed for '{}': {e}",
                            table_name,
                        );
                    }
                }
            }
        }
    }

    async fn cleanup_orphaned_dbsp_tables(&self, view_name: &str) -> anyhow::Result<()> {
        let pattern = format!("__turso_internal_dbsp_state_v%_{}", view_name);
        let check_sql = format!(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '{}'",
            pattern
        );
        let orphaned = self.db_handle.query(&check_sql, HashMap::new()).await?;
        for row in orphaned {
            if let Some(Value::String(table_name)) = row.get("name") {
                tracing::debug!(
                    "[MatviewManager] Cleaning up orphaned DBSP state table: {}",
                    table_name
                );
                self.db_handle
                    .execute_ddl(&format!("DROP TABLE IF EXISTS {}", table_name))
                    .await?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace_and_lowercases() {
        let stored = "CREATE MATERIALIZED VIEW current_focus AS\nSELECT\n    nc.region,\n    nh.block_id\nFROM navigation_cursor nc\nJOIN navigation_history nh ON nc.history_id = nh.id";
        let desired = "CREATE MATERIALIZED VIEW current_focus AS SELECT nc.region, nh.block_id FROM navigation_cursor nc JOIN navigation_history nh ON nc.history_id = nh.id";
        assert_eq!(normalize_view_sql(stored), normalize_view_sql(desired));
    }

    #[test]
    fn normalize_strips_trailing_semicolon() {
        assert_eq!(
            normalize_view_sql("SELECT 1;"),
            normalize_view_sql("SELECT 1")
        );
    }

    #[test]
    fn normalize_detects_actual_change() {
        let v1 = "CREATE MATERIALIZED VIEW foo AS SELECT id FROM block";
        let v2 = "CREATE MATERIALIZED VIEW foo AS SELECT id, content FROM block";
        assert_ne!(normalize_view_sql(v1), normalize_view_sql(v2));
    }

    /// Boot-time idempotency + crash recovery for `reconcile_named_view`.
    ///
    /// 1. First reconcile creates the matview.
    /// 2. A second reconcile with the SAME SELECT is a no-op (`Ok(false)`) — the
    ///    happy path every restart takes; it must never error with "already
    ///    exists" (the stale-DB boot-loop bug).
    /// 3. With an orphaned `__turso_internal_dbsp_state_*` table injected (the
    ///    residue a crash mid-DDL leaves behind) a definition change still
    ///    reconciles cleanly instead of boot-looping. The base table `src` — the
    ///    source of truth — is never dropped.
    #[tokio::test]
    async fn reconcile_named_view_is_idempotent_and_recovers_from_orphaned_state() {
        use crate::turso::TursoBackend;

        let (_backend, handle) = TursoBackend::new_in_memory()
            .await
            .expect("in-memory backend");
        handle
            .execute_ddl("CREATE TABLE IF NOT EXISTS src (id TEXT PRIMARY KEY, v TEXT)")
            .await
            .expect("create base table");
        handle
            .transition_to_ready()
            .await
            .expect("transition to ready");

        // 1. First boot creates the view.
        let created = reconcile_named_view(&handle, "src_view", "SELECT id, v FROM src")
            .await
            .expect("first reconcile");
        assert!(created, "first reconcile should create the view");

        // 2. Second boot with identical SELECT is a no-op — the regression guard
        //    against "table src_view already exists" boot-looping on restart.
        let created_again = reconcile_named_view(&handle, "src_view", "SELECT id, v FROM src")
            .await
            .expect("second reconcile must not error on an existing matview");
        assert!(!created_again, "identical reconcile should be a no-op");

        // 3. Simulate the crash residue: a leftover object occupying the
        //    matview's NAME with no committed `type='view'` row (what makes a
        //    plain `CREATE MATERIALIZED VIEW` die with "already exists" and
        //    boot-loop). A plain table standing on the name reproduces the
        //    collision; `reconcile_named_view` must recover, not fail.
        handle
            .execute_ddl("CREATE TABLE IF NOT EXISTS orphan_view (id TEXT)")
            .await
            .expect("inject orphaned backing table on the matview name");
        let recovered = reconcile_named_view(&handle, "orphan_view", "SELECT id FROM src")
            .await
            .expect("reconcile must recover from a name collision left by a crash");
        assert!(recovered, "collision recovery should (re)create the view");
        let as_view = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='view' AND name='orphan_view'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("query view presence after recovery");
        assert_eq!(as_view.len(), 1, "orphan_view must now be a real matview");

        // Base table survived the whole dance — no user data loss.
        let rows = handle
            .query(
                "SELECT name FROM sqlite_master WHERE type='table' AND name='src'",
                std::collections::HashMap::new(),
            )
            .await
            .expect("query base table presence");
        assert_eq!(rows.len(), 1, "base table `src` must survive matview recovery");

        handle.shutdown().await.expect("shutdown");
    }
}

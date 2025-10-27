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
use tokio::sync::{broadcast, mpsc};

use crate::storage::turso::priority;
use crate::storage::turso::{RowChange, RowChangeStream};
use crate::storage::types::StorageEntity;
use crate::storage::{DbHandle, Resource, extract_table_refs, parse_sql};
use crate::util::strip_order_by;
use holon_api::{BatchWithMetadata, Value};

/// Normalize a SQL statement for comparison: collapse whitespace, strip trailing
/// semicolons, lowercase keywords. This lets us compare `sqlite_master.sql` against
/// the desired CREATE statement without false positives from formatting differences.
fn normalize_view_sql(sql: &str) -> String {
    sql.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .trim_end_matches(';')
        .to_lowercase()
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

    db_handle.execute_ddl(&create_sql).await?;
    tracing::info!(
        "[reconcile_named_view] View '{}' created/updated",
        view_name
    );
    Ok(true)
}

/// Hook called after an FDW cache table is primed with data.
/// Implementations can subscribe to resource notifications, update state, etc.
#[async_trait]
pub trait MatviewHook: Send + Sync {
    /// Called after a successful FDW prime query. `cache_table` is the primed table
    /// (e.g. `"cc_message"`), `fdw_sql` is the executed query including WHERE clause.
    async fn on_fdw_primed(&self, cache_table: &str, fdw_sql: &str);
}

/// Result of watching a query — initial data + CDC stream.
pub struct WatchResult {
    pub initial_rows: Vec<StorageEntity>,
    pub stream: RowChangeStream,
    pub view_name: String,
}

/// Command sent to the CDC demultiplexer task.
enum DemuxCommand {
    /// Register a new subscriber for a specific view.
    Subscribe {
        view_name: String,
        tx: mpsc::Sender<BatchWithMetadata<RowChange>>,
    },
}

/// Manages the full lifecycle of Turso materialized views.
///
/// CDC routing uses a single demultiplexer task instead of spawning one filter
/// task per `subscribe_cdc()` call. The demux task reads from the broadcast
/// channel and routes batches to registered subscribers by `relation_name`.
/// Closed subscribers are pruned automatically.
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
                            Some(DemuxCommand::Subscribe { view_name, tx }) => {
                                tracing::info!("[Demux] Registered subscriber for '{}'", view_name);
                                subscribers.entry(view_name).or_default().push(tx);
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
                                if batch.inner.items.len() > 0 {
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
                                                tracing::warn!(
                                                    "[MatviewManager] CDC subscriber for '{}' is full, dropping batch",
                                                    view_name
                                                );
                                                true // keep subscriber, just drop this batch
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
    /// and prunes closed subscribers automatically.
    pub fn subscribe_cdc(&self, view_name: &str) -> RowChangeStream {
        let (tx, rx) = mpsc::channel(1024);
        tracing::info!("[MatviewManager] subscribe_cdc('{}')", view_name);
        if let Err(e) = self.demux_cmd_tx.try_send(DemuxCommand::Subscribe {
            view_name: view_name.to_string(),
            tx,
        }) {
            tracing::error!(
                "[MatviewManager] Failed to register CDC subscriber for '{}': {}",
                view_name,
                e
            );
        }
        tokio_stream::wrappers::ReceiverStream::new(rx)
    }

    /// Ensure a materialized view exists, query its initial data, and subscribe to CDC.
    #[tracing::instrument(skip(self, sql))]
    pub async fn watch(&self, sql: &str) -> Result<WatchResult> {
        let view_name = self.ensure_view(sql).await?;
        let stream = self.subscribe_cdc(&view_name);
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
}

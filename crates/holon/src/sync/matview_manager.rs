//! Manages materialized view lifecycle — creation, existence checks,
//! orphan cleanup, CDC subscription, and querying.
//!
//! Consolidates the matview lifecycle that was previously duplicated across
//! `BackendEngine::preload_views`, `BackendEngine::watch_query`, and `WatchedQuery::new`.

use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use anyhow::{Context, Result};
use tokio::sync::{broadcast, mpsc};

use crate::storage::turso::priority;
use crate::storage::turso::{RowChange, RowChangeStream};
use crate::storage::types::StorageEntity;
use crate::storage::{DbHandle, Resource, extract_table_refs, parse_sql};
use crate::util::strip_order_by;
use holon_api::{BatchWithMetadata, Value};

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
}

impl MatviewManager {
    pub fn new(db_handle: DbHandle, ddl_mutex: Arc<tokio::sync::Mutex<()>>) -> Self {
        let demux_cmd_tx = Self::spawn_demux(db_handle.cdc_broadcast().clone());
        Self {
            db_handle,
            demux_cmd_tx,
            ddl_mutex,
        }
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
        tokio::spawn(async move {
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
                if let Err(e) = self.db_handle.execute_ddl(&drop_sql).await {
                    tracing::warn!("[MatviewManager] Failed to drop {}: {}", name, e);
                }
                self.cleanup_orphaned_dbsp_tables(name).await;
            }
        }

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
    /// Steps: check existence → acquire DDL mutex → double-check → clean orphaned
    /// DBSP state tables → strip ORDER BY → CREATE MATERIALIZED VIEW with dependency tracking.
    #[tracing::instrument(skip(self, sql), fields(view_name = tracing::field::Empty))]
    pub async fn ensure_view(&self, sql: &str) -> Result<String> {
        let view_name = Self::compute_view_name(sql);
        tracing::Span::current().record("view_name", view_name.as_str());

        if self.view_exists(&view_name).await {
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

        if self.view_exists(&view_name).await {
            tracing::debug!(
                "[MatviewManager] View {} was created while waiting for DDL mutex, reusing",
                view_name
            );
            return Ok(view_name);
        }

        self.cleanup_orphaned_dbsp_tables(&view_name).await;

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

        tracing::debug!("[MatviewManager] Successfully created view: {}", view_name);
        Ok(view_name)
    }

    /// Like `ensure_view` but retries on transient errors (for startup preloading).
    ///
    /// Logs warnings instead of failing — a preload failure is non-fatal because
    /// `watch_query` will create the view lazily later.
    pub async fn preload(&self, sql: &str) -> Result<String> {
        let view_name = Self::compute_view_name(sql);

        if self.view_exists(&view_name).await {
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
    #[tracing::instrument(skip(self))]
    pub async fn query_view(&self, view_name: &str) -> Result<Vec<StorageEntity>> {
        let select_sql = format!("SELECT * FROM {}", view_name);
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
        let check_sql = format!(
            "SELECT name FROM sqlite_master WHERE type='view' AND name='{}'",
            view_name
        );
        match self.db_handle.query(&check_sql, HashMap::new()).await {
            Ok(results) => !results.is_empty(),
            Err(_) => false,
        }
    }

    async fn cleanup_orphaned_dbsp_tables(&self, view_name: &str) {
        let pattern = format!("__turso_internal_dbsp_state_v%_{}", view_name);
        let check_sql = format!(
            "SELECT name FROM sqlite_master WHERE type='table' AND name LIKE '{}'",
            pattern
        );
        if let Ok(orphaned) = self.db_handle.query(&check_sql, HashMap::new()).await {
            for row in orphaned {
                if let Some(Value::String(table_name)) = row.get("name") {
                    tracing::debug!(
                        "[MatviewManager] Cleaning up orphaned DBSP state table: {}",
                        table_name
                    );
                    let _ = self
                        .db_handle
                        .execute_ddl(&format!("DROP TABLE IF EXISTS {}", table_name))
                        .await;
                }
            }
        }
    }
}

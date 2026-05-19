//! Outbound projection from Loro to the SQL sink (Loro → projection).
//!
//! Loro is the single authority. It is seeded from the bundled Org assets via
//! intents (`BlockOrdering::create_in_tree`) and mutated by every writer
//! Loro-first (org reconciler, chord ops). This controller projects Loro → SQL
//! ONLY; there is no SQL→Loro direction — no Turso-seed, no streaming mirror,
//! no inbound EventBus consumer. SQL (`block_raw`) is a pure projection.
//!
//! ## One loop, one direction
//!
//! Any change to the Loro doc — a local edit, a peer `doc.import(&delta)`, or a
//! background `.loro` file load — fires `doc.subscribe_root`, which wakes the
//! controller. Each wake drives one outbound reconcile: [`LoroProjection`]
//! diffs the current Loro snapshot against the sink's current state and writes
//! only the genuinely-changed rows (compare-and-skip), so re-projecting an
//! unchanged snapshot is a no-op.
//!
//! ## Diff strategy
//!
//! `before` = the SQL sink's own current state (via [`SinkReader`]); `after` =
//! the live Loro snapshot. The projection emits the create/update/delete ops
//! that turn `before` into `after`. No persistent block projection is kept in
//! memory; the `Frontiers` watermark only advances the reconcile cursor.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Context, Result};
use loro::{Frontiers, LoroDoc};
use tokio::sync::{Notify, RwLock};
use tracing::{error, info, warn};

use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_api::{EntityName, Value};

use crate::api::snapshot_blocks_from_doc;
use crate::core::datasource::OperationProvider;
use crate::storage::BLOCK_WRITE_TABLE;
use crate::storage::turso::DbHandle;
use crate::sync::LoroDocumentStore;
use crate::sync::event_bus::EventOrigin;

/// Filename of the sidecar file that persists the sync watermark next to the
/// `.loro` snapshot. One file per `LoroDocumentStore`.
pub const SIDECAR_FILENAME: &str = "holon_tree.loro.sync";

/// Bidirectional sync between Loro and the abstract command/event bus.
pub struct LoroSyncController {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    /// The downstream Loro→SQL projection (consolidator → sink convergent
    /// feed). The controller drives it from its run loop on every Loro
    /// change; org's initial scan drives the same instance synchronously via
    /// [`DownstreamProjection::flush`]. Shared so both callers serialize on
    /// the projection's internal lock and advance one `last_synced` watermark.
    projection: Arc<LoroProjection>,
    /// Frontiers watermark — the doc state after the last successful outbound
    /// reconcile. Updated ONLY by the projection, never by
    /// `on_inbound_event`. This ensures peer imports that land concurrently
    /// with inbound event processing are always captured by the next
    /// `fork_at`-based diff. Shared Arc with `projection.last_synced` (the
    /// handle exposes it via `last_synced_frontiers`).
    last_synced: Arc<StdMutex<Frontiers>>,
    wake: Arc<Notify>,
    error_count: Arc<AtomicUsize>,
}

/// Lifetime handle returned by `start()`. Dropping it cancels the background
/// task and the Loro subscription. Tests inspect the controller state through
/// the accessors on the handle.
pub struct LoroSyncControllerHandle {
    /// Kept alive so the Loro callback keeps firing.
    _subscription: loro::Subscription,
    /// Kept alive so the loop keeps running. The inner task takes ownership
    /// of the controller; dropping the JoinHandle does not cancel the task,
    /// so we rely on `wake.notify_one()` being the only input signal — when
    /// this handle is dropped the task will eventually stall waiting on
    /// `wake` and `event_rx` and can be reclaimed at process shutdown.
    _task: tokio::task::JoinHandle<()>,
    /// The shared `block` matview feed (`LiveData`). Held only to keep its CDC
    /// subscribe actor alive for other consumers (the reactive cache) — the
    /// runtime SQL→Loro mirror that used to consume it is retired (Loro is the
    /// authority; see the module doc).
    _block_live: Arc<crate::sync::live_data::LiveData<Block>>,
    last_synced: Arc<StdMutex<Frontiers>>,
    error_count: Arc<AtomicUsize>,
    /// Allows tests to trigger a reconciliation cycle without mutating Loro.
    wake: Arc<Notify>,
}

impl LoroSyncControllerHandle {
    /// Current watermark. May lag `oplog_frontiers()` briefly during
    /// reconciliation; tests should call `wait_for_quiescence` before
    /// asserting on downstream state.
    pub fn last_synced_frontiers(&self) -> Frontiers {
        self.last_synced.lock().unwrap().clone()
    }

    /// Number of errors the controller has logged since startup. Used by the
    /// bridge invariant `I3 — no silent drops`.
    pub fn error_count(&self) -> usize {
        self.error_count.load(Ordering::SeqCst)
    }

    /// Fire a synthetic wake. Used by tests that want to force a reconcile
    /// pass without touching the doc.
    pub fn wake(&self) {
        self.wake.notify_one();
    }
}

impl LoroSyncController {
    pub fn new(doc_store: Arc<RwLock<LoroDocumentStore>>, projection: Arc<LoroProjection>) -> Self {
        let last_synced = projection.last_synced();
        Self {
            doc_store,
            projection,
            last_synced,
            wake: Arc::new(Notify::new()),
            error_count: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Start the controller.
    ///
    /// 1. Subscribe to EventBus synchronously (mirrors `LoroReverseSyncAdapter::start`).
    /// 2. Register `doc.subscribe_root` synchronously so subsequent imports
    ///    queue `wake` notifications.
    /// 3. Fire one synthetic wake so the first loop iteration reconciles any
    ///    offline drift between the persisted watermark and the currently
    ///    loaded Loro state.
    /// 4. Spawn the `tokio::select!` loop on `self`.
    pub async fn start(
        self,
        block_live: Arc<crate::sync::live_data::LiveData<Block>>,
    ) -> Result<LoroSyncControllerHandle> {
        // (1) Loro subscription — synchronous, before spawn.
        let wake_for_callback = self.wake.clone();
        let doc_arc = {
            let store = self.doc_store.read().await;
            let collab = store
                .get_global_doc()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to get global doc: {}", e))?;
            collab.doc()
        };
        let subscription = {
            let doc = &*doc_arc;
            doc.subscribe_root(Arc::new(move |_event| {
                wake_for_callback.notify_one();
            }))
        };

        // (3) Synthetic initial wake so the loop picks up startup drift.
        self.wake.notify_one();

        // Capture handles for the returned LoroSyncControllerHandle.
        let last_synced = self.last_synced.clone();
        let error_count = self.error_count.clone();
        let wake = self.wake.clone();

        // (3) There is NO SQL→Loro mirror. Loro is the authority, seeded from
        // the bundled Org assets via intents (`BlockOrdering::create_in_tree`);
        // SQL (`block_raw`) is a pure projection written by this controller's
        // outbound `on_loro_changed`. A streaming SQL→Loro reflector was a
        // feedback loop that fought Loro authority (transient SQL retractions
        // deleted Loro-held nodes; stale feed inserts re-created deleted ones)
        // and was load-bearing for ordering in non-obvious ways — removed.

        // (4) Spawn the main loop.
        let task = tokio::spawn(async move {
            self.run_loop().await;
        });

        Ok(LoroSyncControllerHandle {
            _subscription: subscription,
            _task: task,
            _block_live: block_live,
            last_synced,
            error_count,
            wake,
        })
    }

    async fn run_loop(self) {
        info!("[LoroSyncController] Started outbound Loro→SQL reconcile loop");
        // The only input is `wake` — fired by the Loro `subscribe_root` callback
        // on every doc change (local writes, peer imports, mirror upserts). Each
        // wake drives one outbound reconcile. The SQL→Loro direction is owned by
        // `run_block_mirror`; there is no inbound EventBus consumer.
        loop {
            self.wake.notified().await;
            if let Err(e) = self.on_loro_changed().await {
                self.error_count.fetch_add(1, Ordering::SeqCst);
                error!("[LoroSyncController] Outbound reconcile failed: {}", e);
            }
        }
    }

    // -- Outbound (Loro → CommandBus) --------------------------------------

    /// Drive the downstream Loro→SQL projection. Delegates to the shared
    /// [`LoroProjection`] (the same instance org's initial scan flushes), which
    /// serializes concurrent callers and owns the `last_synced` watermark.
    async fn on_loro_changed(&self) -> Result<()> {
        self.projection.project().await
    }
}

/// Read side of the downstream sink, used by [`LoroProjection`] as the diff
/// "before". Abstracts the concrete sink so the production Turso path and the
/// in-memory PBT stub share one projection. Returns the current persisted block
/// state keyed by stable id.
#[async_trait::async_trait]
pub trait SinkReader: Send + Sync {
    async fn read_blocks(&self) -> Result<HashMap<String, Block>>;
}

/// Production [`SinkReader`]: reads the `block_raw` base table directly — NOT the
/// `block` matview, which can lag `block_raw` under IVM and would make the
/// projection see stale state and re-emit redundant writes. Tags are hydrated
/// from the `block_tags` junction; `requires` is not read because it is not part
/// of the block equivalence relation (`blocks_differ`).
pub struct TursoSinkReader {
    db_handle: DbHandle,
}

impl TursoSinkReader {
    pub fn new(db_handle: DbHandle) -> Self {
        Self { db_handle }
    }
}

#[async_trait::async_trait]
impl SinkReader for TursoSinkReader {
    async fn read_blocks(&self) -> Result<HashMap<String, Block>> {
        let sql = format!(
            "SELECT b.id, b.parent_id, b.sort_key, b.content, b.content_type, \
                    b.source_language, b.source_name, b.properties, b.marks, \
                    b.created_at, b.updated_at, \
                    COALESCE((SELECT json_group_array(tag) FROM block_tags \
                              WHERE block_id = b.id), '[]') AS tags \
             FROM {table} b",
            table = BLOCK_WRITE_TABLE,
        );
        let rows = self
            .db_handle
            .query(&sql, HashMap::new())
            .await
            .map_err(|e| anyhow::anyhow!("TursoSinkReader query failed: {e}"))?;
        let mut out = HashMap::with_capacity(rows.len());
        for row in rows {
            let block = Block::try_from(row)
                .map_err(|e| anyhow::anyhow!("TursoSinkReader: Block::try_from row: {e}"))?;
            out.insert(block.id.to_string(), block);
        }
        Ok(out)
    }
}

/// The downstream Loro→SQL projection (consolidator → SQL sink convergent
/// feed). Holds exactly what the projection needs — independent of the
/// controller's run-loop task — so it can be constructed and driven without
/// starting the controller. Both the controller run loop and the org initial
/// scan share one instance (via `Arc`) so they advance a single `last_synced`
/// watermark and serialize on `project_lock`.
pub struct LoroProjection {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    /// Shared with `LoroSyncController.last_synced`.
    last_synced: Arc<StdMutex<Frontiers>>,
    command_bus: Arc<dyn OperationProvider>,
    /// Read side of the sink — reads the *current* persisted block state as the
    /// diff "before". The projection compares Loro (authority) against this and
    /// emits only genuinely-changed rows (compare-and-skip), so re-projecting an
    /// unchanged snapshot is a no-op regardless of any watermark position. This
    /// is what makes the sink a convergent feed. A trait so the production Turso
    /// sink and the in-memory test stub share one projection.
    sink_reader: Arc<dyn SinkReader>,
    sidecar_path: PathBuf,
    /// Serializes concurrent `project()` calls (controller run loop vs org
    /// flush) so two callers can't both fork at the same watermark and emit
    /// overlapping diffs.
    project_lock: tokio::sync::Mutex<()>,
    /// Delete-pass gate. `false` until the Loro authority is fully seeded from
    /// the persistent store (`seed_loro_from_persistent_store` →
    /// [`Self::arm`]). The Loro→SQL projection's DELETE pass deletes sink rows
    /// absent from Loro — which is only correct once Loro is the *complete*
    /// authority. During bootstrap the org initial scan flushes the projection
    /// (via `OrgSyncController::on_file_changed`) before the seed has mirrored
    /// raw-inserted layout blocks (`seed_default_layout`'s journals /
    /// root-layout / sidebar) into Loro; an unarmed projection emits creates +
    /// updates but withholds deletes, so those SQL-only seed rows survive until
    /// the seed reconciles them into Loro. Creates/updates are never gated.
    armed: Arc<AtomicBool>,
}

impl LoroProjection {
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        last_synced: Arc<StdMutex<Frontiers>>,
        command_bus: Arc<dyn OperationProvider>,
        sink_reader: Arc<dyn SinkReader>,
        sidecar_path: PathBuf,
    ) -> Self {
        Self {
            doc_store,
            last_synced,
            command_bus,
            sink_reader,
            sidecar_path,
            project_lock: tokio::sync::Mutex::new(()),
            armed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Arm the projection's DELETE pass. Called once, after
    /// `seed_loro_from_persistent_store` has mirrored every persistent-store
    /// block (including the raw-inserted seed layout) into Loro, so that Loro
    /// is now the complete authority and deletes of sink-only rows are
    /// legitimate. Idempotent.
    pub fn arm(&self) {
        self.armed.store(true, Ordering::SeqCst);
    }

    /// Build a projection from a storage directory, loading the `last_synced`
    /// watermark from the sidecar (last session's synced frontier). Loro's own
    /// persisted snapshot is the startup source of truth, so this watermark
    /// correctly bounds the first `project()` diff to changes made since the
    /// last session (e.g. org-scan `create_in_tree` blocks).
    pub fn from_storage(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        command_bus: Arc<dyn OperationProvider>,
        sink_reader: Arc<dyn SinkReader>,
        storage_dir: &std::path::Path,
    ) -> Self {
        let sidecar_path = storage_dir.join(SIDECAR_FILENAME);
        let last_synced = Arc::new(StdMutex::new(load_sidecar_blocking(&sidecar_path)));
        Self::new(
            doc_store,
            last_synced,
            command_bus,
            sink_reader,
            sidecar_path,
        )
    }

    /// The shared `last_synced` watermark Arc. `LoroSyncController` holds the
    /// same Arc so the handle's `last_synced_frontiers` accessor reflects every
    /// projection (run-loop or org-flush).
    pub fn last_synced(&self) -> Arc<StdMutex<Frontiers>> {
        self.last_synced.clone()
    }

    /// Project the Loro doc (the authority) onto the SQL sink, writing only
    /// genuinely-changed rows. The ONLY writer of the `block_raw` rows in Loro
    /// mode, and a *convergent* feed: the diff "before" is the sink's own
    /// current state (`block_raw`), not a watermark-fork of Loro. Comparing
    /// against the sink itself means re-projecting an unchanged snapshot emits
    /// zero ops regardless of any frontier position — so there is no bootstrap
    /// cycle to police and no race between the seed and concurrent creates.
    pub async fn project(&self) -> Result<()> {
        let _guard = self.project_lock.lock().await;
        let t0 = std::time::Instant::now();

        let current = self.current_frontiers().await?;

        // "after" = Loro authority. "before" = the SQL sink's current state.
        let doc_arc = self.raw_doc().await?;
        let after: HashMap<String, Block> = {
            let doc = &*doc_arc;
            snapshot_blocks_from_doc(doc)
        };
        let before: HashMap<String, Block> = self.read_sql_snapshot().await?;
        let snapshot_ms = t0.elapsed().as_millis();

        let mut ops = diff_snapshots_to_ops(&before, &after);

        // Delete-pass gate: until the projection is armed (Loro fully seeded
        // from the persistent store), withhold deletes so the bootstrap org
        // scan's flush can't delete raw-inserted seed-layout rows that the
        // seed hasn't mirrored into Loro yet. Creates/updates always flow.
        if !self.armed.load(Ordering::SeqCst) {
            let before_len = ops.len();
            ops.retain(|(name, _)| name != "delete");
            let withheld = before_len - ops.len();
            if withheld > 0 {
                tracing::warn!(
                    "[LoroProjection] unarmed: withholding {} delete(s) until Loro authority is seeded",
                    withheld
                );
            }
        }

        if !ops.is_empty() {
            // Surface the aggregate_ids of each outbound op so we can
            // bisect Loro-create → block_raw-write drops by grepping the
            // log for the failing block id.
            let op_summary: Vec<String> = ops
                .iter()
                .map(|(op_name, params)| {
                    let id = params
                        .get("id")
                        .and_then(|v| v.as_string())
                        .unwrap_or("<no-id>");
                    format!("{}:{}", op_name, id)
                })
                .collect();
            tracing::trace!(
                "[LoroSyncController OUTBOUND] before={} after={} ops={} aggregate_ids={:?}",
                before.len(),
                after.len(),
                ops.len(),
                op_summary,
            );
            let op_count = op_summary.len();
            self.command_bus
                .execute_batch_with_origin(&EntityName::new("block"), ops, EventOrigin::Loro)
                .await
                .map_err(|e| anyhow::anyhow!("execute_batch_with_origin failed: {}", e))?;
            // Startup-promptness timing: how long a projection pass took to read
            // the Loro+SQL snapshots and apply the diff. During the boot burst
            // this surfaces whether per-pass snapshot cost (O(blocks)) is the
            // bottleneck behind slow first-paint.
            tracing::info!(
                "[LoroProjection] applied {} op(s) in {}ms (snapshot {}ms, after={} before={})",
                op_count,
                t0.elapsed().as_millis(),
                snapshot_ms,
                after.len(),
                before.len(),
            );
        }

        // Advance the watermark. This is the ONLY place last_synced is
        // updated — on_inbound_event deliberately does NOT touch it, so
        // concurrent peer imports are always captured by the next diff.
        *self.last_synced.lock().unwrap() = current;
        self.persist_sidecar().await?;
        Ok(())
    }

    async fn raw_doc(&self) -> Result<Arc<LoroDoc>> {
        let store = self.doc_store.read().await;
        let collab = store
            .get_global_doc()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get global doc: {}", e))?;
        Ok(collab.doc())
    }

    async fn current_frontiers(&self) -> Result<Frontiers> {
        let doc_arc = self.raw_doc().await?;
        let doc = &*doc_arc;
        Ok(doc.oplog_frontiers())
    }

    /// Read the sink's current block state (the diff "before") via the injected
    /// [`SinkReader`].
    async fn read_sql_snapshot(&self) -> Result<HashMap<String, Block>> {
        self.sink_reader.read_blocks().await
    }

    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    async fn persist_sidecar(&self) -> Result<()> {
        if let Some(parent) = self.sidecar_path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("create sidecar parent dir {}", parent.display()))?;
        }
        let bytes = self.last_synced.lock().unwrap().encode();
        std::fs::write(&self.sidecar_path, bytes)
            .with_context(|| format!("write sidecar {}", self.sidecar_path.display()))?;
        Ok(())
    }

    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    async fn persist_sidecar(&self) -> Result<()> {
        // wasm32 demo is in-memory; no sidecar persistence.
        Ok(())
    }
}

#[async_trait::async_trait]
impl holon_core::DownstreamProjection for LoroProjection {
    async fn flush(&self) -> holon_core::traits::Result<()> {
        self.project()
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> { format!("{e:#}").into() })
    }
}

// -- Sidecar helpers -------------------------------------------------------

fn load_sidecar_blocking(path: &std::path::Path) -> Frontiers {
    match std::fs::read(path) {
        Ok(bytes) => match Frontiers::decode(&bytes) {
            Ok(f) => {
                info!(
                    "[LoroSyncController] Loaded sidecar from {} ({} bytes)",
                    path.display(),
                    bytes.len()
                );
                f
            }
            Err(e) => {
                warn!(
                    "[LoroSyncController] Sidecar at {} exists but is corrupt ({}); \
                     starting with empty watermark.",
                    path.display(),
                    e
                );
                Frontiers::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            info!(
                "[LoroSyncController] No sidecar at {} — starting with empty watermark",
                path.display()
            );
            Frontiers::default()
        }
        Err(e) => {
            warn!(
                "[LoroSyncController] Failed to read sidecar {}: {}",
                path.display(),
                e
            );
            Frontiers::default()
        }
    }
}

pub(crate) fn is_empty_frontiers(f: &Frontiers) -> bool {
    f == &Frontiers::default()
}

// -- Block snapshot diff → command-bus ops ---------------------------------

pub(crate) fn diff_snapshots_to_ops(
    before: &HashMap<String, Block>,
    after: &HashMap<String, Block>,
) -> Vec<(String, HashMap<String, Value>)> {
    let mut ops: Vec<(String, HashMap<String, Value>)> = Vec::new();

    // Creates (in "after" but not in "before").
    // Emit in an order where parents come before children: walk "after" in
    // topological order by following parent_id chains. Blocks whose parent
    // is not in "after" go first (they're the roots).
    let creates: Vec<&Block> = after
        .values()
        .filter(|b| !before.contains_key(b.id.as_str()))
        .collect();
    let ordered_creates = topological_sort_creates(creates, after);
    for block in ordered_creates {
        tracing::trace!(
            "[LORO_DIFF_TRACE] CREATE id={} content={:?}",
            block.id,
            block.content
        );
        ops.push(("create".to_string(), block_to_params(block)));
    }

    // Updates (in both, but differ).
    //
    // Build a delta params map containing only fields that actually changed
    // between old and new. This prevents the outbound from overwriting SQL
    // fields that didn't change in Loro — if content didn't change, it
    // simply won't be in the SET clause.
    //
    // Phase 2 authority flip: ALL `_expected_*` watermark gating is gone.
    // `SqlBlockOperations::set_field` routes block field writes through
    // `BlockCellRegistry::write_field` (Loro), and the outbound projector
    // is the only path that writes block columns to SQL. With one writer
    // per field there's no concurrent direct SQL dispatch to regress
    // against, so the compare-and-set guards are dead weight. The diff
    // guard at the end of `prepare_update` (`AND (col1 IS NOT val1 OR
    // …)`) still keeps no-op UPDATEs from firing spurious CDC.
    for (id, new_block) in after {
        if let Some(old_block) = before.get(id)
            && blocks_differ(old_block, new_block)
        {
            let params = block_diff_params(old_block, new_block);
            tracing::trace!(
                "[LORO_DIFF_TRACE] UPDATE id={} content_before={:?} content_after={:?}",
                id,
                old_block.content,
                new_block.content
            );
            ops.push(("update".to_string(), params));
        }
    }

    // Deletes (in "before" but not in "after"). Delete leaves first so
    // parent pointers stay consistent during the batch.
    let deletes: Vec<&Block> = before
        .values()
        .filter(|b| !after.contains_key(b.id.as_str()))
        .collect();
    let ordered_deletes = topological_sort_deletes(deletes, before);
    for block in ordered_deletes {
        let mut params = HashMap::new();
        params.insert("id".to_string(), Value::String(block.id.to_string()));
        ops.push(("delete".to_string(), params));
    }

    ops
}

/// Topologically sort creates so parents precede children.
fn topological_sort_creates<'a>(
    creates: Vec<&'a Block>,
    all: &'a HashMap<String, Block>,
) -> Vec<&'a Block> {
    let create_ids: std::collections::HashSet<String> =
        creates.iter().map(|b| b.id.to_string()).collect();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result: Vec<&Block> = Vec::new();

    fn visit<'a>(
        block: &'a Block,
        all: &'a HashMap<String, Block>,
        create_ids: &std::collections::HashSet<String>,
        visited: &mut std::collections::HashSet<String>,
        result: &mut Vec<&'a Block>,
    ) {
        let id = block.id.to_string();
        if visited.contains(&id) {
            return;
        }
        visited.insert(id);
        let parent_id = block.parent_id.as_str();
        if create_ids.contains(parent_id)
            && let Some(parent) = all.get(parent_id)
        {
            visit(parent, all, create_ids, visited, result);
        }
        result.push(block);
    }

    for block in &creates {
        visit(block, all, &create_ids, &mut visited, &mut result);
    }
    result
}

/// Topologically sort deletes so children precede parents (leaves first).
fn topological_sort_deletes<'a>(
    deletes: Vec<&'a Block>,
    all: &'a HashMap<String, Block>,
) -> Vec<&'a Block> {
    let mut creates_order = topological_sort_creates(deletes.clone(), all);
    creates_order.reverse();
    creates_order
}

pub fn block_to_params(block: &Block) -> HashMap<String, Value> {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(block.id.to_string()));
    params.insert(
        "parent_id".to_string(),
        Value::String(block.parent_id.to_string()),
    );
    params.insert("content".to_string(), Value::String(block.content.clone()));
    params.insert(
        "content_type".to_string(),
        Value::String(block.content_type.to_string()),
    );

    let now = chrono::Utc::now().timestamp_millis();
    let created = if block.created_at > 0 {
        block.created_at
    } else {
        now
    };
    params.insert("created_at".to_string(), Value::Integer(created));
    params.insert("updated_at".to_string(), Value::Integer(now));
    params.insert(
        "sort_key".to_string(),
        Value::String(block.sort_key.clone()),
    );

    if !block.tags.is_empty() {
        let arr: Vec<Value> = block
            .tags
            .iter()
            .map(|t| Value::String(t.clone()))
            .collect();
        params.insert("tags".to_string(), Value::Array(arr));
    }

    // `requires` is an edge field (`block_requires` junction). Emit it as a
    // typed Array — the SQL provider's edge partition routes it to the junction.
    // Without this, an org-edna dependency created in Loro mode never reaches
    // SQL (the projection's create dropped it). Mirrors `tags` above.
    if !block.requires.is_empty() {
        let arr: Vec<Value> = block
            .requires
            .iter()
            .map(|r| Value::String(r.clone()))
            .collect();
        params.insert("requires".to_string(), Value::Array(arr));
    }

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
        // `_source_header_args` rides into params via the
        // `block.properties` flatten below. Don't also write a
        // no-underscore copy — that landed in the `properties` JSON column
        // alongside the underscore form, polluted `drawer_properties()`,
        // and made `Block::get_source_header_args` (which reads the
        // underscore key) the only canonical reader.
    }

    // Flatten all raw block properties onto the top-level params map. The
    // downstream `OperationProvider` (e.g. `SqlOperationProvider`) partitions
    // them into SQL columns vs. the `properties` JSON column based on its own
    // `known_columns` table. The Loro side never has to know which fields are
    // first-class columns.
    for (k, v) in &block.properties {
        // Edge-typed fields (`tags`, `requires`) live in junction tables and
        // are emitted via their dedicated params/paths — never as flattened
        // properties. A stray edge key in `properties` (data pollution) would
        // otherwise reach SqlOperationProvider's edge partition as a non-Array
        // and panic. Skip them.
        if k == "tags" || k == "requires" {
            continue;
        }
        params.entry(k.clone()).or_insert_with(|| v.clone());
    }

    // Project Block.marks → SQL `marks` TEXT column as a JSON string. None →
    // omit (NULL); Some(empty or non-empty) → JSON-encode. The SQL column
    // discriminator is `marks IS NOT NULL`.
    if let Some(ref marks) = block.marks {
        params.insert(
            "marks".to_string(),
            Value::String(holon_api::marks_to_json(marks)),
        );
    }

    params
}

/// Build a params map containing only fields that differ between `old` and
/// `new`, plus the `id` (always needed for the WHERE clause) and `updated_at`.
/// This prevents Loro outbound reconcile from overwriting SQL fields that a
/// concurrent direct write has already advanced.
fn block_diff_params(old: &Block, new: &Block) -> HashMap<String, Value> {
    let mut params = HashMap::new();
    params.insert("id".to_string(), Value::String(new.id.to_string()));

    let now = chrono::Utc::now().timestamp_millis();
    params.insert("updated_at".to_string(), Value::Integer(now));

    if old.parent_id != new.parent_id {
        params.insert(
            "parent_id".to_string(),
            Value::String(new.parent_id.to_string()),
        );
    }
    if old.content != new.content {
        params.insert("content".to_string(), Value::String(new.content.clone()));
    }
    if old.content_type != new.content_type {
        params.insert(
            "content_type".to_string(),
            Value::String(new.content_type.to_string()),
        );
    }
    if old.tags != new.tags {
        let arr: Vec<Value> = new.tags.iter().map(|t| Value::String(t.clone())).collect();
        params.insert("tags".to_string(), Value::Array(arr));
    }
    if old.requires != new.requires {
        let arr: Vec<Value> = new
            .requires
            .iter()
            .map(|r| Value::String(r.clone()))
            .collect();
        params.insert("requires".to_string(), Value::Array(arr));
    }
    if old.source_language != new.source_language
        && let Some(ref lang) = new.source_language
    {
        params.insert(
            "source_language".to_string(),
            Value::String(lang.to_string()),
        );
    }
    if old.source_name != new.source_name
        && let Some(ref name) = new.source_name
    {
        params.insert("source_name".to_string(), Value::String(name.clone()));
    }
    if old.sort_key != new.sort_key {
        params.insert("sort_key".to_string(), Value::String(new.sort_key.clone()));
    }
    if old.properties_map() != new.properties_map() {
        for (k, v) in &new.properties {
            // Edge-typed fields (`tags`, `requires`) live in junction tables and
            // are emitted via their dedicated Array params above — never as a
            // flattened property. A stray edge key in `properties` (legacy data
            // pollution) would otherwise reach SqlOperationProvider's edge
            // partition as a non-Array and panic. Skip them. Mirrors the same
            // guard in `block_to_params`.
            if k == "tags" || k == "requires" {
                continue;
            }
            params.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
    if old.marks != new.marks {
        // `None` → emit Value::Null so prepare_update writes `marks = NULL`.
        // `Some` → emit JSON-encoded marks.
        let val = match &new.marks {
            Some(marks) => Value::String(holon_api::marks_to_json(marks)),
            None => Value::Null,
        };
        params.insert("marks".to_string(), val);
    }

    params
}

/// Snapshot a shared LoroDoc into topo-sorted SQL create ops.
///
/// `patch_block` is called on each `Block` before conversion to params —
/// callers use it to remap `parent_id` (shared root → mount URI) and
/// stamp properties like `shared-tree-id`.
pub(crate) fn project_shared_doc_to_ops(
    shared_doc: &LoroDoc,
    patch_block: impl Fn(&mut Block),
) -> Vec<(String, HashMap<String, Value>)> {
    let mut blocks = snapshot_blocks_from_doc(shared_doc);
    for block in blocks.values_mut() {
        patch_block(block);
    }
    diff_snapshots_to_ops(&HashMap::new(), &blocks)
}

fn blocks_differ(a: &Block, b: &Block) -> bool {
    a.content != b.content
        || a.parent_id != b.parent_id
        || a.content_type != b.content_type
        || a.source_language != b.source_language
        || a.source_name != b.source_name
        || a.tags != b.tags
        || a.sort_key != b.sort_key
        || a.properties_map() != b.properties_map()
        || a.marks != b.marks
}

#[cfg(test)]
mod marks_outbound_tests {
    //! Phase 1.3 follow-up: cover the Loro→SQL outbound projection of marks.
    //!
    //! These are pure-function tests over `block_to_params` /
    //! `block_diff_params` / `blocks_differ` — no Loro/SQL runtime needed.
    //! End-to-end Loro→SQL behavior is already covered in
    //! `loro_backend::tests::marks_round_trip_through_loro` (read path).

    use super::*;
    use holon_api::{EntityUri, InlineMark, MarkSpan};

    fn block_with_marks(content: &str, marks: Option<Vec<MarkSpan>>) -> Block {
        let mut b = Block::new_text(
            EntityUri::block("b1"),
            EntityUri::no_parent(),
            content.to_string(),
        );
        b.marks = marks;
        b
    }

    #[test]
    fn block_to_params_emits_marks_when_present() {
        let block = block_with_marks(
            "hello world",
            Some(vec![MarkSpan::new(0, 5, InlineMark::Bold)]),
        );
        let params = block_to_params(&block);
        let marks_val = params.get("marks").expect("marks param present");
        let s = marks_val.as_string().expect("marks is a String");
        // Canonical JSON is the wire format; not validating exact bytes here,
        // just that it parses back to the same Vec.
        let parsed: Vec<MarkSpan> = holon_api::marks_from_json(s).expect("parse");
        assert_eq!(parsed, vec![MarkSpan::new(0, 5, InlineMark::Bold)]);
    }

    #[test]
    fn block_to_params_omits_marks_when_none() {
        let block = block_with_marks("plain text", None);
        let params = block_to_params(&block);
        assert!(
            !params.contains_key("marks"),
            "marks key should be absent when Block.marks=None"
        );
    }

    #[test]
    fn block_diff_params_emits_marks_when_changed() {
        let old = block_with_marks("hi", None);
        let new = block_with_marks("hi", Some(vec![MarkSpan::new(0, 2, InlineMark::Italic)]));
        let params = block_diff_params(&old, &new);
        let marks_val = params.get("marks").expect("marks change emitted");
        let s = marks_val.as_string().expect("marks is a String");
        let parsed: Vec<MarkSpan> = holon_api::marks_from_json(s).expect("parse");
        assert_eq!(parsed, vec![MarkSpan::new(0, 2, InlineMark::Italic)]);
    }

    #[test]
    fn block_diff_params_emits_null_when_marks_cleared() {
        let old = block_with_marks("hi", Some(vec![MarkSpan::new(0, 2, InlineMark::Bold)]));
        let new = block_with_marks("hi", None);
        let params = block_diff_params(&old, &new);
        let marks_val = params.get("marks").expect("marks change emitted");
        assert_eq!(
            *marks_val,
            Value::Null,
            "expected Null sentinel for cleared marks"
        );
    }

    #[test]
    fn block_diff_params_omits_marks_when_unchanged() {
        let m = vec![MarkSpan::new(0, 2, InlineMark::Bold)];
        let old = block_with_marks("hi", Some(m.clone()));
        let new = block_with_marks("hi", Some(m));
        let params = block_diff_params(&old, &new);
        assert!(
            !params.contains_key("marks"),
            "no marks key when marks identical; got {params:?}"
        );
    }

    #[test]
    fn blocks_differ_detects_marks_change() {
        let none_block = block_with_marks("hi", None);
        let some_block = block_with_marks("hi", Some(vec![MarkSpan::new(0, 2, InlineMark::Bold)]));
        assert!(blocks_differ(&none_block, &some_block));
        assert!(blocks_differ(&some_block, &none_block));
    }

    #[test]
    fn blocks_differ_ignores_identical_marks() {
        let m = vec![MarkSpan::new(0, 2, InlineMark::Bold)];
        let a = block_with_marks("hi", Some(m.clone()));
        let b = block_with_marks("hi", Some(m));
        assert!(!blocks_differ(&a, &b));
    }

    // Phase 2 authority flip: `_expected_marks` watermark gating dropped
    // alongside `_expected_parent_id` and `_expected_content`. The diff
    // snapshot still emits new marks values when they change (asserted in
    // `block_diff_params_emits_marks_when_changed` above); it just no
    // longer adds compare-and-set guards because there's a single SQL
    // writer (`on_loro_changed`) per field.
    #[test]
    fn diff_snapshots_no_longer_emits_expected_marks_guard() {
        let mut before = HashMap::new();
        let mut after = HashMap::new();

        let id = "block:b1".to_string();
        let old_marks = vec![MarkSpan::new(0, 5, InlineMark::Bold)];
        let new_marks = vec![MarkSpan::new(0, 5, InlineMark::Italic)];
        before.insert(
            id.clone(),
            block_with_marks("hello", Some(old_marks.clone())),
        );
        after.insert(
            id.clone(),
            block_with_marks("hello", Some(new_marks.clone())),
        );

        let ops = diff_snapshots_to_ops(&before, &after);
        let (_, params) = ops
            .iter()
            .find(|(op, _)| op == "update")
            .expect("update op");

        assert!(
            !params.contains_key("_expected_marks"), // ALLOW(deleted_cell_symbol): test asserts the watermark name is ABSENT post-authority-flip
            "watermark dropped post-authority-flip; got params: {params:?}"
        );
        let new_val = params.get("marks").expect("new marks present");
        let s = new_val.as_string().expect("marks is String");
        let parsed: Vec<MarkSpan> = holon_api::marks_from_json(s).expect("parse new");
        assert_eq!(parsed, new_marks);
    }
}

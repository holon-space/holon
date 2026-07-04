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

use holon_api::EdgeField;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::types::ContentType;

use crate::LoroDocumentStore;
use crate::loro_backend::{
    SnapshotBlock, snapshot_blocks_from_doc, snapshot_blocks_from_doc_settled,
};
use crate::{BaseKey, BaseStore};
use holon_core::OriginTaggedWrites;

/// Filename of the sidecar file that persists the sync watermark next to the
/// `.loro` snapshot. One file per `LoroDocumentStore`.
pub const SIDECAR_FILENAME: &str = "holon_tree.loro.sync";

/// Whether the O(changed) incremental Loro→SQL projection fast path is enabled.
/// Default OFF: the incremental path is a spike pending correctness hardening on
/// rare create/move/delete sequences (the composed keystone occasionally tripped
/// a `SplitBlock` "Block not found"). With it off the projection takes the
/// baseline full-snapshot-and-diff path, which is the shipped behaviour.
/// Set `HOLON_LORO_INCREMENTAL_PROJECTION=1` (or `true`/`on`/`yes`) to opt in.
fn incremental_projection_enabled() -> bool {
    std::env::var("HOLON_LORO_INCREMENTAL_PROJECTION")
        .map(|v| matches!(v.as_str(), "1" | "true" | "TRUE" | "yes" | "on"))
        .unwrap_or(false)
}

/// Above this many pending facts in one drain, the incremental fast path defers
/// to a full reseed: one bulk `snapshot_blocks_from_doc_settled` is cheaper than
/// draining thousands of facts and re-reading each node (cold org-scan / bulk
/// import), and it bounds the accumulator. Floored against `live.len()` so small
/// vaults still take the fast path for modest batches. Heuristic — tune with the
/// `crdt_incr_bench` at scale.
const INCREMENTAL_BATCH_MAX: usize = 512;

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
    _block_live: Arc<holon_api::live_data::LiveData<Block>>,
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
        block_live: Arc<holon_api::live_data::LiveData<Block>>,
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
        // Event-driven incremental input: extract each commit's dirty facts on
        // the committing thread (a pure function of the event — no `doc` access,
        // no checkout) and append them to the projection's shared queue. The run
        // loop drains it. This replaces re-deriving the delta via
        // `doc.diff(last, current)`, which checked the shared live doc out.
        let pending_for_callback = self.projection.pending();
        let subscription = {
            let doc = &*doc_arc;
            doc.subscribe_root(Arc::new(move |event| {
                let mut facts = crate::loro_backend::extract_pending_changes(&event);
                if !facts.is_empty() {
                    pending_for_callback.lock().unwrap().append(&mut facts);
                }
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
        // on every doc change (local writes, peer imports). Each wake drives one
        // outbound reconcile. There is no SQL→Loro mirror and no inbound EventBus
        // consumer; Loro is the authority and this loop is its sole projector.
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
    async fn read_blocks(&self) -> Result<HashMap<String, SnapshotBlock>>;
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
    /// The pinned block consolidator — the single owner of the block sink-write.
    /// The projection hands it the Loro-vs-base diff as a typed intent
    /// `ChangeSet`; it records the intent (op-multiset agreement) and writes the
    /// SQL sink. (Phase 5: replaces the projection's direct
    /// `execute_batch_with_origin` block call.)
    consolidator: Arc<crate::consolidator::BlockConsolidator>,
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
    /// Delete-pass gate. `false` until the Loro authority is fully seeded (from
    /// the bundled Org assets via `create_in_tree` intents) → [`Self::arm`].
    /// The Loro→SQL projection's DELETE pass deletes sink rows
    /// absent from Loro — which is only correct once Loro is the *complete*
    /// authority. During bootstrap the org initial scan flushes the projection
    /// (via `FileSyncController::on_file_changed`) before the seed has mirrored
    /// raw-inserted layout blocks (`seed_default_layout`'s journals /
    /// root-layout / sidebar) into Loro; an unarmed projection emits creates +
    /// updates but withholds deletes, so those SQL-only seed rows survive until
    /// the seed reconciles them into Loro. Creates/updates are never gated.
    armed: Arc<AtomicBool>,
    /// The last-projected block snapshot, kept live in memory and mutated
    /// **in place** by the incremental fast path (O(changed) per commit). It is
    /// the diff "before" — replacing the former per-pass full-document snapshot
    /// + `SyncBaseStore` sidecar. Seeded by a full reseed on cold boot (and any
    /// unsettled/unarmed reseed pass); steady-state edits mutate only the
    /// changed keys. Persistence is no longer needed: on restart the snapshot is
    /// rebuilt from the loaded `.loro` (the authority) and reconciled once
    /// against the SQL sink.
    live: StdMutex<HashMap<String, SnapshotBlock>>,
    /// `true` once `live` has been seeded by at least one full reseed. Until
    /// then every pass takes the full path (cold-boot reconcile against SQL).
    seeded: AtomicBool,
    /// `TreeID -> stable id` for every live node, maintained by the incremental
    /// path so a deleted node — whose Loro meta may already be gone — can still
    /// be mapped to the sink row to delete. Rebuilt on each full reseed.
    tid_index: StdMutex<HashMap<loro::TreeID, String>>,
    /// The last-projected snapshot (persisted sidecar), used ONLY by the default
    /// full-projection path (`HOLON_LORO_INCREMENTAL_PROJECTION` off). Baseline
    /// diff "before". The incremental path ignores it and uses `live` instead.
    base_store: crate::SyncBaseStore,
    /// Event-driven incremental input: the `subscribe_root` callback extracts the
    /// dirty facts of each commit (`extract_pending_changes`) and appends them
    /// here on the committing thread. `project()` drains the whole queue and reads
    /// the CURRENT tree for the named nodes — replacing `doc.diff(last, current)`,
    /// which checked the shared live doc out and raced concurrent readers. Shared
    /// `Arc` so `LoroSyncController::start` can hand the same queue to the
    /// callback. Only the incremental fast path consumes it.
    pending: Arc<StdMutex<Vec<crate::loro_backend::PendingChange>>>,
}

impl LoroProjection {
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        last_synced: Arc<StdMutex<Frontiers>>,
        command_bus: Arc<dyn OriginTaggedWrites>,
        sink_reader: Arc<dyn SinkReader>,
        sidecar_path: PathBuf,
    ) -> Self {
        let base_store = crate::SyncBaseStore::from_frontiers_sidecar(&sidecar_path);
        // A `LoroProjection` exists only in the Loro-present config (it IS the
        // Loro→SQL projection), so the consolidator is pinned to Loro.
        let caps = crate::capability::SessionCapabilities::detect_and_pin(true);
        let consolidator = Arc::new(crate::consolidator::BlockConsolidator::new(
            command_bus,
            caps,
        ));
        Self {
            doc_store,
            last_synced,
            consolidator,
            sink_reader,
            sidecar_path,
            project_lock: tokio::sync::Mutex::new(()),
            armed: Arc::new(AtomicBool::new(false)),
            live: StdMutex::new(HashMap::new()),
            seeded: AtomicBool::new(false),
            tid_index: StdMutex::new(HashMap::new()),
            base_store,
            pending: Arc::new(StdMutex::new(Vec::new())),
        }
    }

    /// The shared pending-facts queue. `LoroSyncController::start` hands this to
    /// the `subscribe_root` callback, which appends `extract_pending_changes` of
    /// each commit. `project()`'s incremental fast path drains it.
    pub fn pending(&self) -> Arc<StdMutex<Vec<crate::loro_backend::PendingChange>>> {
        self.pending.clone()
    }

    /// Whether the pending-facts queue is currently empty. Exposed so a settle
    /// detector can, if it wants concurrent-commit settle-correctness, require an
    /// empty queue in addition to `last_synced == oplog_frontiers` (see the
    /// drain-protocol note in `project()`). Not consumed by the keystone.
    pub fn pending_is_empty(&self) -> bool {
        self.pending.lock().unwrap().is_empty()
    }

    /// Phase 2 shadow counters `(agreements, divergences)`: how many projection
    /// batches' emitted ops decoded to a `ChangeSet` that agreed with / diverged
    /// from the source op multiset. The gate requires `divergences == 0`.
    pub fn shadow_changeset_counters(&self) -> (usize, usize) {
        self.consolidator.shadow_counters()
    }

    /// Arm the projection's DELETE pass. Called once, after the seed (bundled
    /// Org assets via `create_in_tree` intents, incl. the raw-inserted seed
    /// layout) has populated Loro, so that Loro is now the complete authority
    /// and deletes of sink-only rows are legitimate. Idempotent.
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
        command_bus: Arc<dyn OriginTaggedWrites>,
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
    /// mode. The diff "before" is the last projected Loro snapshot read through
    /// the `BaseStore` seam (the 3-way base), NOT the sink's own current state;
    /// the SQL sink is consulted only as a cold-boot seed when the base is
    /// unseeded. Diffing against a stable base means re-projecting an unchanged
    /// snapshot emits zero ops regardless of any frontier position — no
    /// bootstrap cycle to police, no race between the seed and concurrent
    /// creates.
    pub async fn project(&self) -> Result<()> {
        let _guard = self.project_lock.lock().await;
        let t0 = std::time::Instant::now();

        let doc_arc = self.raw_doc().await?;
        let current = {
            let doc = &*doc_arc;
            doc.oplog_frontiers()
        };
        let last = self.last_synced.lock().unwrap().clone();
        let incremental = incremental_projection_enabled();
        let seeded = self.seeded.load(Ordering::SeqCst);
        let armed = self.armed.load(Ordering::SeqCst);

        // ── Incremental fast path — O(changed), event-driven, no checkout ─────
        // GATED behind `HOLON_LORO_INCREMENTAL_PROJECTION` (default OFF). When on
        // and seeded+armed, drain the pending-facts queue (populated by the
        // `subscribe_root` callback via `extract_pending_changes`) and read only
        // the named nodes from the CURRENT tree. This replaces
        // `doc.diff(last, current)`, which checked the shared live doc out (to
        // `last`, then `current`, then restored) and raced concurrent readers —
        // the root cause of the flaky `SplitBlock … Block not found`.
        if incremental && seeded && armed {
            // Drain the WHOLE queue first — never early-return while facts are
            // pending (that would silently drop a committed change).
            let pending: Vec<crate::loro_backend::PendingChange> =
                std::mem::take(&mut *self.pending.lock().unwrap());

            // Idle wake: no facts and the oplog hasn't moved → nothing to do.
            if pending.is_empty() && last == current {
                return Ok(());
            }

            // Take the O(changed) path only for a bounded, event-supplied batch.
            // An empty queue with a moved frontier (pre-subscription boot window,
            // a filtered Checkout event, or a reseed-race) or an oversized batch
            // (cold org-scan / bulk import — one snapshot beats draining thousands
            // of facts + re-reading each node) routes to the full reseed below.
            // Both routes are checkout-free, so neither can reintroduce the race.
            let live_len = self.live.lock().unwrap().len();
            let take_incremental =
                !pending.is_empty() && pending.len() <= INCREMENTAL_BATCH_MAX.max(live_len);
            if take_incremental {
                let (changed, settled) = {
                    let doc = &*doc_arc;
                    let mut tid_index = self.tid_index.lock().unwrap();
                    crate::loro_backend::incremental_block_changes(doc, &pending, &mut tid_index)?
                };
                if settled {
                    let (ops, before_len, after_len) = {
                        let mut live = self.live.lock().unwrap();
                        let before_len = live.len();
                        let mut ops: Vec<(String, holon_api::StorageEntity)> = Vec::new();
                        for (id, new) in changed {
                            match new {
                                Some(nb) => match live.get(&id) {
                                    None => {
                                        ops.push(("create".to_string(), block_to_params(&nb)));
                                        live.insert(id, nb);
                                    }
                                    Some(old) if blocks_differ(old, &nb) => {
                                        ops.push((
                                            "update".to_string(),
                                            block_diff_params(old, &nb),
                                        ));
                                        live.insert(id, nb);
                                    }
                                    Some(_) => { /* identical — no-op (compare-and-skip) */ }
                                },
                                None => {
                                    if live.remove(&id).is_some() {
                                        let mut params = holon_api::StorageEntity::new();
                                        params.insert("id".into(), Value::String(id));
                                        ops.push(("delete".to_string(), params));
                                    }
                                }
                            }
                        }
                        let after_len = live.len();
                        (ops, before_len, after_len)
                    };
                    let snapshot_ms = t0.elapsed().as_millis();
                    return self
                        .emit_ops(
                            ops,
                            current,
                            &t0,
                            snapshot_ms,
                            after_len,
                            before_len,
                            "incremental",
                        )
                        .await;
                }
                tracing::warn!(
                    "[LoroProjection] incremental pass unsettled; reseeding from full snapshot"
                );
            }
            // Not a bounded incremental batch (or unsettled) → drop through to
            // the full reseed path below, which reads current state (no checkout).
        }

        // ── Full projection path (DEFAULT; base_store-based, baseline-exact) ──
        // "after" = the full Loro authority snapshot. "before" = the persisted
        // base (baseline diff source), seeded from the SQL sink on cold boot. In
        // incremental mode after the first seed, `live` is the authoritative
        // last-emitted state, so an incremental→full reseed diffs against it.
        let (after, after_settled): (HashMap<String, SnapshotBlock>, bool) = {
            let doc = &*doc_arc;
            snapshot_blocks_from_doc_settled(doc)
        };
        let base_key = BaseKey::global();
        let was_seeded = self.base_store.is_base_seeded(&base_key);
        let before: Arc<HashMap<String, SnapshotBlock>> = if incremental && seeded {
            Arc::new(self.live.lock().unwrap().clone())
        } else if was_seeded {
            self.base_store.get_base(&base_key)
        } else {
            Arc::new(self.read_sql_snapshot().await?)
        };
        let snapshot_ms = t0.elapsed().as_millis();

        let mut ops = diff_snapshots_to_ops(&before, &after);
        let had_changes = !ops.is_empty();

        // Delete-pass gate. Withhold deletes when the projection is not yet armed
        // (Loro still seeding — raw-inserted seed-layout rows not yet mirrored) or
        // the snapshot is unsettled (a live node was transiently meta-incomplete,
        // so a still-live block looks "deleted"). Creates / updates always flow.
        if !armed || !after_settled {
            let n = ops.len();
            ops.retain(|(name, _)| name != "delete");
            let withheld = n - ops.len();
            if withheld > 0 {
                tracing::warn!(
                    "[LoroProjection] withholding {} delete(s) (armed={}, snapshot_settled={})",
                    withheld,
                    armed,
                    after_settled,
                );
            }
        }

        let before_len = before.len();
        let after_len = after.len();

        // Commit the new base state — only on a settled snapshot.
        if after_settled {
            if incremental {
                // Seed / refresh the incremental state so a later gated pass can
                // take the fast path (and so an incremental→full reseed diffs
                // against `live`).
                let idx = {
                    let doc = &*doc_arc;
                    crate::loro_backend::build_tid_index(doc)
                };
                *self.tid_index.lock().unwrap() = idx;
                *self.live.lock().unwrap() = after;
                self.seeded.store(true, Ordering::SeqCst);
                // This full snapshot captured everything up to `current`, so any
                // facts accumulated before/during it are now stale — drop them so
                // the next incremental pass starts clean (bounds the queue during
                // the pre-arm boot window and after every reseed).
                self.pending.lock().unwrap().clear();
            } else if had_changes || !was_seeded {
                // Baseline: advance the persisted base; skip the doc-sized rewrite
                // when the diff was empty against an already-seeded base.
                self.base_store.put_base(&base_key, after);
            }
        }

        self.emit_ops(
            ops,
            current,
            &t0,
            snapshot_ms,
            after_len,
            before_len,
            "full",
        )
        .await
    }

    /// Apply the diff ops through the consolidator and advance the watermark.
    /// Shared by the incremental fast path and the full reseed path.
    async fn emit_ops(
        &self,
        ops: Vec<(String, holon_api::StorageEntity)>,
        current: Frontiers,
        t0: &std::time::Instant,
        snapshot_ms: u128,
        after_len: usize,
        before_len: usize,
        mode: &str,
    ) -> Result<()> {
        if !ops.is_empty() {
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
                "[LoroSyncController OUTBOUND] mode={} after={} before={} ops={} aggregate_ids={:?}",
                mode,
                after_len,
                before_len,
                ops.len(),
                op_summary,
            );
            let op_count = op_summary.len();

            let base_ref = {
                let bytes = self.last_synced.lock().unwrap().encode();
                Some(
                    bytes
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>(),
                )
            };
            let provenance = holon_api::Provenance {
                command_id: None,
                base_ref,
            };
            self.consolidator.apply(ops, provenance).await?;
            tracing::info!(
                "[LoroProjection] applied {} op(s) in {}ms (snapshot {}ms, after={} before={}) [{}]",
                op_count,
                t0.elapsed().as_millis(),
                snapshot_ms,
                after_len,
                before_len,
                mode,
            );
            tracing::debug!(
                target: "holon_latency",
                stage = "projection",
                ops = op_count,
                blocks = after_len,
                snapshot_ms = snapshot_ms as u64,
                ms = t0.elapsed().as_millis() as u64,
                mode = mode,
                "holon_latency",
            );
        }

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

    /// Read the sink's current block state (the diff "before") via the injected
    /// [`SinkReader`].
    async fn read_sql_snapshot(&self) -> Result<HashMap<String, SnapshotBlock>> {
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
    before: &HashMap<String, SnapshotBlock>,
    after: &HashMap<String, SnapshotBlock>,
) -> Vec<(String, holon_api::StorageEntity)> {
    let mut ops: Vec<(String, holon_api::StorageEntity)> = Vec::new();

    // Creates (in "after" but not in "before").
    // Emit in an order where parents come before children: walk "after" in
    // topological order by following parent_id chains. Blocks whose parent
    // is not in "after" go first (they're the roots).
    let creates: Vec<&SnapshotBlock> = after
        .values()
        .filter(|s| !before.contains_key(s.block.id.as_str()))
        .collect();
    let ordered_creates = topological_sort_creates(creates, after);
    for snap in ordered_creates {
        tracing::trace!(
            "[LORO_DIFF_TRACE] CREATE id={} content={:?}",
            snap.block.id,
            snap.block.content
        );
        ops.push(("create".to_string(), block_to_params(snap)));
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
                old_block.block.content,
                new_block.block.content
            );
            ops.push(("update".to_string(), params));
        }
    }

    // Deletes (in "before" but not in "after"). Delete leaves first so
    // parent pointers stay consistent during the batch.
    let deletes: Vec<&SnapshotBlock> = before
        .values()
        .filter(|s| !after.contains_key(s.block.id.as_str()))
        .collect();
    let ordered_deletes = topological_sort_deletes(deletes, before);
    for snap in ordered_deletes {
        let mut params = HashMap::new();
        params.insert("id".into(), Value::String(snap.block.id.to_string()));
        ops.push(("delete".to_string(), params));
    }

    ops
}

/// Topologically sort creates so parents precede children.
fn topological_sort_creates<'a>(
    creates: Vec<&'a SnapshotBlock>,
    all: &'a HashMap<String, SnapshotBlock>,
) -> Vec<&'a SnapshotBlock> {
    let create_ids: std::collections::HashSet<String> =
        creates.iter().map(|s| s.block.id.to_string()).collect();
    let mut visited: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut result: Vec<&SnapshotBlock> = Vec::new();

    fn visit<'a>(
        snap: &'a SnapshotBlock,
        all: &'a HashMap<String, SnapshotBlock>,
        create_ids: &std::collections::HashSet<String>,
        visited: &mut std::collections::HashSet<String>,
        result: &mut Vec<&'a SnapshotBlock>,
    ) {
        let id = snap.block.id.to_string();
        if visited.contains(&id) {
            return;
        }
        visited.insert(id);
        let parent_id = snap.block.parent_id.as_str();
        if create_ids.contains(parent_id)
            && let Some(parent) = all.get(parent_id)
        {
            visit(parent, all, create_ids, visited, result);
        }
        result.push(snap);
    }

    for snap in &creates {
        visit(snap, all, &create_ids, &mut visited, &mut result);
    }
    result
}

/// Topologically sort deletes so children precede parents (leaves first).
fn topological_sort_deletes<'a>(
    deletes: Vec<&'a SnapshotBlock>,
    all: &'a HashMap<String, SnapshotBlock>,
) -> Vec<&'a SnapshotBlock> {
    let mut creates_order = topological_sort_creates(deletes.clone(), all);
    creates_order.reverse();
    creates_order
}

pub fn block_to_params(snap: &SnapshotBlock) -> holon_api::StorageEntity {
    let block = &snap.block;
    let mut params = holon_api::StorageEntity::new();
    params.insert("id".into(), Value::String(block.id.to_string()));
    params.insert(
        "parent_id".into(),
        Value::String(block.parent_id.to_string()),
    );
    params.insert("content".into(), Value::String(block.content.clone()));
    params.insert(
        "content_type".into(),
        Value::String(block.content_type.to_string()),
    );

    let now = holon_api::clock::now_millis();
    let created = if block.created_at > 0 {
        block.created_at
    } else {
        now
    };
    params.insert("created_at".into(), Value::Integer(created));
    params.insert("updated_at".into(), Value::Integer(now));
    // Projection-totality guard (R-1): every owned Loro block MUST have a
    // non-empty fractional_index. An empty sort_key means the projection
    // failed to carry the Loro authority's order — the "sort_key stays A0"
    // bug class. This debug_assert fires on the bug; the PBT invariant
    // `SELECT count(*) FROM block_raw WHERE sort_key IS NULL` fires in CI.
    debug_assert!(
        !snap.sort_key.is_empty(),
        "projection-totality violation: block {} has empty sort_key",
        block.id
    );
    params.insert("sort_key".into(), Value::String(snap.sort_key.clone()));

    // Edge fields (`block_tags`/`block_requires` junctions). Emit each non-empty
    // set as a typed Array — the SQL provider's edge partition routes it to the
    // junction table. Iterating `EdgeField::ALL` (rather than hand-listing) is
    // what keeps a newly added edge field from being silently dropped here.
    for field in EdgeField::ALL {
        if !field.is_empty(block) {
            params.insert(field.column().into(), field.param_value(block));
        }
    }

    if block.content_type == ContentType::Source {
        if let Some(ref lang) = block.source_language {
            params.insert("source_language".into(), Value::String(lang.to_string()));
        }
        if let Some(ref name) = block.source_name {
            params.insert("source_name".into(), Value::String(name.clone()));
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
        // Edge-typed fields live in junction tables and are emitted via their
        // dedicated params/paths above — never as flattened properties. A stray
        // edge key in `properties` (data pollution) would otherwise reach
        // SqlOperationProvider's edge partition as a non-Array and panic.
        if EdgeField::is_edge_column(k) {
            continue;
        }
        params.entry(k.as_str().into()).or_insert_with(|| v.clone());
    }

    // Project Block.marks → SQL `marks` TEXT column as a JSON string. None →
    // omit (NULL); Some(empty or non-empty) → JSON-encode. The SQL column
    // discriminator is `marks IS NOT NULL`.
    if let Some(ref marks) = block.marks {
        params.insert(
            "marks".into(),
            Value::String(holon_api::marks_to_json(marks)),
        );
    }

    params
}

/// Build a params map containing only fields that differ between `old` and
/// `new`, plus the `id` (always needed for the WHERE clause) and `updated_at`.
/// This prevents Loro outbound reconcile from overwriting SQL fields that a
/// concurrent direct write has already advanced.
fn block_diff_params(old: &SnapshotBlock, new: &SnapshotBlock) -> holon_api::StorageEntity {
    let (old_sort_key, new_sort_key) = (&old.sort_key, &new.sort_key);
    let old = &old.block;
    let new = &new.block;
    let mut params = HashMap::new();
    params.insert("id".into(), Value::String(new.id.to_string()));

    let now = holon_api::clock::now_millis();
    params.insert("updated_at".into(), Value::Integer(now));

    if old.parent_id != new.parent_id {
        params.insert("parent_id".into(), Value::String(new.parent_id.to_string()));
    }
    if old.content != new.content {
        params.insert("content".into(), Value::String(new.content.clone()));
    }
    if old.content_type != new.content_type {
        params.insert(
            "content_type".into(),
            Value::String(new.content_type.to_string()),
        );
    }
    // Edge fields: emit each that changed. Iterating `EdgeField::ALL` keeps this
    // diff in lockstep with `blocks_differ` — omitting a field here (the H12
    // bug: `requires` compared in one but not the other) is unrepresentable.
    for field in EdgeField::ALL {
        if field.differs(old, new) {
            params.insert(field.column().into(), field.param_value(new));
        }
    }
    // Emit on ANY change, INCLUDING a clear (`Some → None`) — mirror `marks`
    // below (`None → Value::Null`). The former `&& let Some(new) = …` guard
    // could not represent clearing the field, so a Loro clear (e.g. a source
    // block re-typed to text) neither propagated to SQL (the column stayed
    // stale) NOR round-tripped through the intent vocabulary — `blocks_differ`
    // saw the change but the update decoded to zero typed ops, tripping the
    // `agrees_with_ops` divergence counter. Keeps this diff in lockstep with
    // `blocks_differ`, which compares these fields with plain `!=`.
    if old.source_language != new.source_language {
        params.insert(
            "source_language".into(),
            match &new.source_language {
                Some(lang) => Value::String(lang.to_string()),
                None => Value::Null,
            },
        );
    }
    if old.source_name != new.source_name {
        params.insert(
            "source_name".into(),
            match &new.source_name {
                Some(name) => Value::String(name.clone()),
                None => Value::Null,
            },
        );
    }
    if old_sort_key != new_sort_key {
        params.insert("sort_key".into(), Value::String(new_sort_key.clone()));
    }
    if old.properties_map() != new.properties_map() {
        for (k, v) in &new.properties {
            // Edge-typed fields live in junction tables and are emitted via
            // their dedicated Array params above — never as a flattened
            // property. A stray edge key in `properties` (legacy data pollution)
            // would otherwise reach SqlOperationProvider's edge partition as a
            // non-Array and panic. Mirrors the guard in `block_to_params`.
            if EdgeField::is_edge_column(k) {
                continue;
            }
            params.entry(k.as_str().into()).or_insert_with(|| v.clone());
        }
        // Emit the `Value::Null` REMOVAL sentinel for every key present in
        // `old` but absent from `new` — mirror the `marks` / `source_language`
        // clear handling above (`Some -> None` => Null). Iterating only
        // `new.properties` could not represent a deletion, so a property
        // removed in Loro left the stale value in SQL's `properties` JSON
        // forever (the base advances to `after`, so it is never re-diffed —
        // one-shot silent data loss). `prepare_update` removes the key on this
        // sentinel; without it the merge only ever inserts.
        for k in old.properties.keys() {
            if EdgeField::is_edge_column(k) {
                continue;
            }
            if !new.properties.contains_key(k) {
                params.entry(k.as_str().into()).or_insert(Value::Null);
            }
        }
    }
    if old.marks != new.marks {
        // `None` → emit Value::Null so prepare_update writes `marks = NULL`.
        // `Some` → emit JSON-encoded marks.
        let val = match &new.marks {
            Some(marks) => Value::String(holon_api::marks_to_json(marks)),
            None => Value::Null,
        };
        params.insert("marks".into(), val);
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
) -> Vec<(String, holon_api::StorageEntity)> {
    let mut blocks = snapshot_blocks_from_doc(shared_doc);
    for snap in blocks.values_mut() {
        patch_block(&mut snap.block);
    }
    diff_snapshots_to_ops(&HashMap::new(), &blocks)
}

fn blocks_differ(a: &SnapshotBlock, b: &SnapshotBlock) -> bool {
    a.sort_key != b.sort_key
        || a.block.content != b.block.content
        || a.block.parent_id != b.block.parent_id
        || a.block.content_type != b.block.content_type
        || a.block.source_language != b.block.source_language
        || a.block.source_name != b.block.source_name
        || EdgeField::ALL.iter().any(|f| f.differs(&a.block, &b.block))
        || a.block.properties_map() != b.block.properties_map()
        || a.block.marks != b.block.marks
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

    fn block_with_marks(content: &str, marks: Option<Vec<MarkSpan>>) -> SnapshotBlock {
        let mut b = Block::new_text(
            EntityUri::block("b1"),
            EntityUri::no_parent(),
            content.to_string(),
        );
        b.marks = marks;
        SnapshotBlock {
            block: b,
            sort_key: "A0".to_string(),
        }
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

    fn block_with_source_lang(lang: Option<holon_api::SourceLanguage>) -> SnapshotBlock {
        let mut b = Block::new_text(
            EntityUri::block("b1"),
            EntityUri::no_parent(),
            "q".to_string(),
        );
        b.source_language = lang;
        SnapshotBlock {
            block: b,
            sort_key: "A0".to_string(),
        }
    }

    /// Regression for the `agrees_with_ops` divergence a parallel keystone soak
    /// found ({create:8, update:2} → reencoded {create:8, update:1}): clearing
    /// `source_language` (`Some → None`) was detected by `blocks_differ` but the
    /// old `let Some(new) = …` guard emitted NO param, so the clear never
    /// reached SQL and the update decoded to zero typed ops. Must now emit
    /// `Null` (mirror `marks`), keeping the diff in lockstep with `blocks_differ`.
    #[test]
    fn block_diff_params_emits_null_when_source_language_cleared() {
        let old = block_with_source_lang(Some(holon_api::SourceLanguage::Render));
        let new = block_with_source_lang(None);
        assert!(
            blocks_differ(&old, &new),
            "blocks_differ must see the clear"
        );
        let params = block_diff_params(&old, &new);
        assert_eq!(
            params.get("source_language"),
            Some(&Value::Null),
            "cleared source_language must emit a Null sentinel (else the clear is \
             dropped from SQL and the update decodes to zero typed ops): {params:?}"
        );
    }

    fn block_with_props(props: &[(&str, &str)]) -> SnapshotBlock {
        let mut b = Block::new_text(
            EntityUri::block("b1"),
            EntityUri::no_parent(),
            "q".to_string(),
        );
        for (k, v) in props {
            b.set_property(*k, Value::String((*v).to_string()));
        }
        SnapshotBlock {
            block: b,
            sort_key: "A0".to_string(),
        }
    }

    /// Regression for the P0 silent data-loss bug: a property present in `old`
    /// but absent from `new` (deleted in Loro) must emit the `Value::Null`
    /// REMOVAL sentinel so `prepare_update` clears the key from SQL's
    /// `properties` JSON. Iterating only `new.properties` dropped deletions —
    /// the stale value lived in SQL forever (mirror of the source_language fix).
    #[test]
    fn block_diff_params_emits_null_when_property_removed() {
        let old = block_with_props(&[("foo", "bar"), ("keep", "me")]);
        let new = block_with_props(&[("keep", "me")]);
        assert!(
            blocks_differ(&old, &new),
            "blocks_differ must see the property removal"
        );
        let params = block_diff_params(&old, &new);
        assert_eq!(
            params.get("foo"),
            Some(&Value::Null),
            "removed property must emit a Null sentinel (else SQL keeps the \
             stale value forever): {params:?}"
        );
    }

    /// The removal update must also round-trip through the typed intent
    /// vocabulary (no `agrees_with_ops` divergence): the Null decodes to a
    /// SetField, so source `update:1` re-encodes to `update:1`.
    #[test]
    fn property_removal_update_agrees_with_ops() {
        use holon_api::{ChangeSet, Provenance, agrees_with_ops};
        let old = block_with_props(&[("foo", "bar")]);
        let new = block_with_props(&[]);
        let ops = vec![("update".to_string(), block_diff_params(&old, &new))];
        let cs = ChangeSet::from_ops(&ops, Provenance::default());
        assert!(
            agrees_with_ops(&cs, &ops).is_ok(),
            "property removal must round-trip: {:?}",
            agrees_with_ops(&cs, &ops)
        );
    }

    /// The end-to-end guard: a source_language-clear update must round-trip
    /// through the typed intent vocabulary (no `agrees_with_ops` divergence).
    #[test]
    fn source_language_clear_update_agrees_with_ops() {
        use holon_api::{ChangeSet, Provenance, agrees_with_ops};
        let old = block_with_source_lang(Some(holon_api::SourceLanguage::Render));
        let new = block_with_source_lang(None);
        let ops = vec![("update".to_string(), block_diff_params(&old, &new))];
        let cs = ChangeSet::from_ops(&ops, Provenance::default());
        assert!(
            agrees_with_ops(&cs, &ops).is_ok(),
            "source_language clear must round-trip: {:?}",
            agrees_with_ops(&cs, &ops)
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

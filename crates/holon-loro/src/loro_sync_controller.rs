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
//! background `.loro` file load — fires `doc.subscribe_root`, which (on the
//! committing thread) extracts the commit's dirty facts
//! (`extract_pending_changes` — a pure function of the event, no `doc` access,
//! no checkout) into a shared queue and wakes the controller. Each wake drives
//! one outbound reconcile via [`LoroProjection`].
//!
//! ## Diff strategy — event-driven, `O(changed)`
//!
//! Steady state is the ONLY steady-state path: drain the pending-facts queue
//! and read just the changed nodes from the CURRENT tree
//! (`incremental_block_changes`), diffing against the in-memory `live` snapshot
//! (the last-emitted state). Cost is proportional to what changed, not the tree
//! size, and nothing checks the shared live doc out — eliminating the torn-walk
//! race behind the flaky `SplitBlock … Block not found`.
//!
//! The full-document walk survives ONLY to (re)seed `live`, in three
//! checkout-free roles: cold-boot seeding (diffing against the SQL sink via
//! [`SinkReader`]), reseed-on-unsettled (a touched node was transiently
//! meta-incomplete), and the unarmed/oversized-batch bootstrap. There is no env
//! switch and no separate persisted base store. The `live` snapshot and the
//! `Frontiers` watermark advance only AFTER the sink write succeeds, so a
//! failed (rolled-back) batch never advances the base ahead of the sink — the
//! next pass reseeds and retries rather than silently dropping the change.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

use anyhow::Context;
use anyhow::Result;
use holon_api::EdgeField;
use holon_api::Value;
use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_core::OriginTaggedWrites;
use loro::Frontiers;
use loro::LoroDoc;
use tokio::sync::Notify;
use tokio::sync::RwLock;
use tracing::error;
use tracing::info;
use tracing::warn;

use crate::LoroDocumentStore;
use crate::loro_backend::SnapshotBlock;
use crate::loro_backend::snapshot_blocks_from_doc;
use crate::loro_backend::snapshot_blocks_from_doc_settled;

/// Filename of the sidecar file that persists the sync watermark next to the
/// `.loro` snapshot. One file per `LoroDocumentStore`.
pub const SIDECAR_FILENAME: &str = "holon_tree.loro.sync";

/// Above this many pending facts in one drain, the incremental fast path defers
/// to a full reseed: one bulk `snapshot_blocks_from_doc_settled` is cheaper
/// than draining thousands of facts and re-reading each node (cold org-scan /
/// bulk import), and it bounds the accumulator. Floored against `live.len()` so
/// small vaults still take the fast path for modest batches. Heuristic — tune
/// with the `crdt_incr_bench` at scale.
const INCREMENTAL_BATCH_MAX: usize = 512;

/// Why a `project()` pass took the full-reseed walk instead of the O(changed)
/// incremental fast path. Threaded from each trigger site to `emit_ops` so the
/// `holon_latency` projection event can attribute every `mode=full` emission to
/// a specific cause — the per-reason telemetry the reseed-latency workstream
/// (BugFunnel row 71) needs to tell legitimate seeds (`coldboot`) apart from
/// the four reseed *leaks* (`empty_pending_moved_frontier`, `unsettled`,
/// `orphan`, `oversized`) and the recovery path (`sink_fail`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FullReason {
    /// Not seeded/armed at entry: cold-boot seed or unarmed bootstrap
    /// reconcile.
    ColdBoot,
    /// Seeded+armed, but the drained queue was empty while the oplog frontier
    /// moved (pre-subscription boot window, filtered Checkout, or reseed race).
    EmptyPendingMovedFrontier,
    /// Seeded+armed, incremental batch read an unsettled (meta-incomplete)
    /// tree.
    Unsettled,
    /// Seeded+armed, incremental batch contained an orphan create (a changed
    /// node's parent absent from the O(changed) batch) → reseed from sink
    /// truth.
    Orphan,
    /// Seeded+armed, but the drained batch exceeded the incremental cap
    /// (`INCREMENTAL_BATCH_MAX.max(live_len)`) — cold org-scan / bulk import.
    Oversized,
    /// A prior pass's incremental sink write failed (batch rolled back); this
    /// pass rebuilds the base from sink truth and retries.
    SinkFail,
}

impl FullReason {
    fn as_str(self) -> &'static str {
        match self {
            FullReason::ColdBoot => "coldboot",
            FullReason::EmptyPendingMovedFrontier => "empty_pending_moved_frontier",
            FullReason::Unsettled => "unsettled",
            FullReason::Orphan => "orphan",
            FullReason::Oversized => "oversized",
            FullReason::SinkFail => "sink_fail",
        }
    }
}

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
    /// 1. Subscribe to EventBus synchronously (mirrors
    ///    `LoroReverseSyncAdapter::start`).
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
    /// The pinned block consolidator — the single owner of the block
    /// sink-write. The projection hands it the Loro-vs-base diff as a typed
    /// intent `ChangeSet`; it records the intent (op-multiset agreement)
    /// and writes the SQL sink. (Phase 5: replaces the projection's direct
    /// `execute_batch_with_origin` block call.)
    consolidator: Arc<crate::consolidator::BlockConsolidator>,
    /// Read side of the sink — reads the *current* persisted block state as the
    /// diff "before". The projection compares Loro (authority) against this and
    /// emits only genuinely-changed rows (compare-and-skip), so re-projecting
    /// an unchanged snapshot is a no-op regardless of any watermark
    /// position. This is what makes the sink a convergent feed. A trait so
    /// the production Turso sink and the in-memory test stub share one
    /// projection.
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
    /// the diff "before" — the sole in-memory projection base. Seeded by a full
    /// reseed on cold boot (and any unsettled/unarmed reseed pass);
    /// steady-state edits mutate only the changed keys. Persistence is not
    /// needed: on restart the snapshot is rebuilt from the loaded `.loro`
    /// (the authority) and reconciled once against the SQL sink.
    live: StdMutex<HashMap<String, SnapshotBlock>>,
    /// `true` once `live` has been seeded by at least one full reseed. Until
    /// then every pass takes the full path (cold-boot reconcile against SQL).
    seeded: AtomicBool,
    /// `TreeID -> stable id` for every live node, maintained by the incremental
    /// path so a deleted node — whose Loro meta may already be gone — can still
    /// be mapped to the sink row to delete. Rebuilt on each full reseed.
    tid_index: StdMutex<HashMap<loro::TreeID, String>>,
    /// Event-driven incremental input: the `subscribe_root` callback extracts
    /// the dirty facts of each commit (`extract_pending_changes`) and
    /// appends them here on the committing thread. `project()` drains the
    /// whole queue and reads the CURRENT tree for the named nodes —
    /// replacing `doc.diff(last, current)`, which checked the shared live
    /// doc out and raced concurrent readers. Shared
    /// `Arc` so `LoroSyncController::start` can hand the same queue to the
    /// callback. Only the incremental fast path consumes it.
    pending: Arc<StdMutex<Vec<crate::loro_backend::PendingChange>>>,
    /// Set when a pass clears `seeded` and RETURNS (the incremental sink-write
    /// failure at `emit_ops` Err) so the *next* pass's full walk — which sees
    /// only `seeded == false` at entry, indistinguishable from cold boot — can
    /// attribute its `mode=full` event to `sink_fail` rather than `coldboot`.
    /// Taken (cleared) by that next full walk. The orphan reseed falls through
    /// in the SAME pass and sets its reason locally, so it does not use this.
    pending_reseed_reason: StdMutex<Option<FullReason>>,
}

impl LoroProjection {
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        last_synced: Arc<StdMutex<Frontiers>>,
        command_bus: Arc<dyn OriginTaggedWrites>,
        sink_reader: Arc<dyn SinkReader>,
        sidecar_path: PathBuf,
    ) -> Self {
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
            pending: Arc::new(StdMutex::new(Vec::new())),
            pending_reseed_reason: StdMutex::new(None),
        }
    }

    /// The shared pending-facts queue. `LoroSyncController::start` hands this
    /// to the `subscribe_root` callback, which appends
    /// `extract_pending_changes` of each commit. `project()`'s incremental
    /// fast path drains it.
    pub fn pending(&self) -> Arc<StdMutex<Vec<crate::loro_backend::PendingChange>>> {
        self.pending.clone()
    }

    /// Whether the pending-facts queue is currently empty. Exposed so a settle
    /// detector can, if it wants concurrent-commit settle-correctness, require
    /// an empty queue in addition to `last_synced == oplog_frontiers` (see
    /// the drain-protocol note in `project()`). Not consumed by the
    /// keystone.
    pub fn pending_is_empty(&self) -> bool {
        self.pending.lock().unwrap().is_empty()
    }

    /// Phase 2 shadow counters `(agreements, divergences)`: how many projection
    /// batches' emitted ops decoded to a `ChangeSet` that agreed with /
    /// diverged from the source op multiset. The gate requires `divergences
    /// == 0`.
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

    /// Test-only view of the incremental diff base (`live`), the atomic
    /// base-advance contract's guarded state.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn live_snapshot(&self) -> std::collections::HashMap<String, SnapshotBlock> {
        self.live.lock().unwrap().clone()
    }

    /// Test-only view of the synced watermark.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn last_synced_value(&self) -> Frontiers {
        self.last_synced.lock().unwrap().clone()
    }

    /// Test-only view of the seeded flag — flipped to `false` by the atomic
    /// base-advance contract when a sink write fails, forcing a full reseed.
    #[cfg(any(test, feature = "test-helpers"))]
    pub fn is_seeded(&self) -> bool {
        self.seeded.load(std::sync::atomic::Ordering::SeqCst)
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
    /// mode. Steady state drains the event-driven pending-facts queue and reads
    /// only the changed nodes (`O(changed)`); the diff "before" is the
    /// in-memory `live` snapshot. The full walk runs only to (re)seed
    /// `live` — cold boot (diffing against the SQL sink),
    /// reseed-on-unsettled, and the unarmed/oversized-batch bootstrap.
    /// Diffing against a stable base means re-projecting an unchanged
    /// snapshot emits zero ops regardless of any frontier position.
    pub async fn project(&self) -> Result<()> {
        let _guard = self.project_lock.lock().await;
        let t0 = std::time::Instant::now();

        let doc_arc = self.raw_doc().await?;
        let current = {
            let doc = &*doc_arc;
            doc.oplog_frontiers()
        };
        let last = self.last_synced.lock().unwrap().clone();
        let seeded = self.seeded.load(Ordering::SeqCst);
        let armed = self.armed.load(Ordering::SeqCst);

        // Reason threaded to `emit_ops` if this pass reaches the full walk.
        // seeded+armed at entry → set at the specific fall-through site below.
        // Not seeded/armed → cold boot, unless a prior pass recorded `sink_fail`
        // when its incremental write failed and returned (taken here). `None`
        // reaching the full walk is a logic error (fails loud via `.expect`).
        let mut full_reason: Option<FullReason> = if seeded && armed {
            None
        } else {
            Some(
                self.pending_reseed_reason
                    .lock()
                    .unwrap()
                    .take()
                    .unwrap_or(FullReason::ColdBoot),
            )
        };

        // ── Incremental fast path — O(changed), event-driven, no checkout ─────
        // The SOLE steady-state projector. Once seeded+armed, drain the
        // pending-facts queue (populated by the `subscribe_root` callback via
        // `extract_pending_changes`) and read only the named nodes from the
        // CURRENT tree. This replaces `doc.diff(last, current)`, which checked the
        // shared live doc out (to `last`, then `current`, then restored) and raced
        // concurrent readers — the root cause of the flaky `SplitBlock … Block not
        // found`. The full walk below survives ONLY for cold-boot seeding,
        // reseed-on-unsettled, and the unarmed/oversized-batch bootstrap.
        if seeded && armed {
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
                    // Build ops + a STAGING plan WITHOUT mutating `live`. `live`
                    // (the diff base) and `last_synced` advance only AFTER the sink
                    // write succeeds — a failed apply (e.g. an FK reject that rolls
                    // the whole batch back) must not advance the base, which would
                    // silently drop the change; instead we reseed and retry.
                    let (ops, staging, before_len, after_len, has_orphan) = {
                        let live = self.live.lock().unwrap();
                        let before_len = live.len();
                        let mut ops: Vec<(String, holon_api::StorageEntity)> = Vec::new();
                        // (id, Some(block)) = insert/update into `live`; (id, None)
                        // = remove from `live`. Applied only on emit_ops success.
                        let mut staging: Vec<(String, Option<SnapshotBlock>)> = Vec::new();
                        let mut creates = 0usize;
                        let mut deletes = 0usize;
                        for (id, new) in changed {
                            match new {
                                Some(nb) => match live.get(&id) {
                                    None => {
                                        ops.push(("create".to_string(), block_to_params(&nb)));
                                        staging.push((id, Some(nb)));
                                        creates += 1;
                                    }
                                    Some(old) if blocks_differ(old, &nb) => {
                                        ops.push((
                                            "update".to_string(),
                                            block_diff_params(old, &nb),
                                        ));
                                        staging.push((id, Some(nb)));
                                    }
                                    Some(_) => { /* identical — no-op (compare-and-skip) */ }
                                },
                                None => {
                                    if live.contains_key(&id) {
                                        let mut params = holon_api::StorageEntity::new();
                                        params.insert("id".into(), Value::String(id.clone()));
                                        ops.push(("delete".to_string(), params));
                                        staging.push((id, None));
                                        deletes += 1;
                                    }
                                }
                            }
                        }
                        let after_len = before_len + creates - deletes;
                        // Orphan-create guard: a create whose parent block is
                        // NEITHER already in the sink base (`live`) NOR created by
                        // this same batch will FK-reject at COMMIT and roll the
                        // whole batch back — silently dropping every co-batched
                        // row. The incremental fast path reads only the CHANGED
                        // nodes, so an ancestor that changed in an earlier, already-
                        // drained interval (whose facts were consumed by a prior
                        // pass) can be missing from this batch while its descendants
                        // are present. When that happens we must NOT emit the
                        // partial batch; fall through to the full reseed below,
                        // which reads the WHOLE current tree and re-emits the
                        // subtree with its parent. (Seed layout / `sentinel`
                        // non-`block:` parents are always satisfiable.)
                        let has_orphan = {
                            let batch_created: std::collections::HashSet<&str> = ops
                                .iter()
                                .filter(|(name, _)| name == "create")
                                .filter_map(|(_, e)| e.get("id").and_then(|v| v.as_string()))
                                .collect();
                            ops.iter().any(|(name, e)| {
                                if name != "create" {
                                    return false;
                                }
                                match e.get("parent_id").and_then(|v| v.as_string()) {
                                    Some(pid) if pid.starts_with("block:") => {
                                        !live.contains_key(pid) && !batch_created.contains(pid)
                                    }
                                    _ => false,
                                }
                            })
                        };
                        (ops, staging, before_len, after_len, has_orphan)
                    };
                    if has_orphan {
                        // Do not commit an FK-doomed partial batch. Force the base
                        // to be rebuilt from SINK TRUTH (`read_sql_snapshot`), not
                        // the in-memory `live` diff base: `live` may itself have
                        // drifted to hold the missing parent while `block_raw` does
                        // not, in which case a reseed diffed against `live` would
                        // re-omit the parent and FK-fail again. Clearing `seeded`
                        // makes the reseed below diff the full tree against the
                        // actual sink, so the parent's create is (re)emitted with
                        // its subtree. (The reseed reads sink truth unconditionally
                        // now, but clearing the flag keeps `seeded` honest — the
                        // in-memory `live` base is not trustworthy until the reseed
                        // re-establishes it.)
                        self.seeded.store(false, Ordering::SeqCst);
                        full_reason = Some(FullReason::Orphan);
                        tracing::warn!(
                            "[LoroProjection] incremental batch has orphan create(s) (a changed \
                             node's parent is absent from this O(changed) batch); reseeding from \
                             sink truth to re-emit the subtree with its parent"
                        );
                    } else {
                        let snapshot_ms = t0.elapsed().as_millis();
                        match self
                            .emit_ops(
                                ops,
                                current,
                                &t0,
                                snapshot_ms,
                                after_len,
                                before_len,
                                "incremental",
                                "incremental",
                            )
                            .await
                        {
                            Ok(()) => {
                                // Sink write committed — now advance the diff base.
                                let mut live = self.live.lock().unwrap();
                                for (id, v) in staging {
                                    match v {
                                        Some(nb) => {
                                            live.insert(id, nb);
                                        }
                                        None => {
                                            live.remove(&id);
                                        }
                                    }
                                }
                                return Ok(());
                            }
                            Err(e) => {
                                // The sink write failed (batch rolled back). Leave
                                // `live`/`last_synced` untouched and force a full
                                // reseed next pass so the base is rebuilt from truth
                                // and the change retried — never silently dropped
                                // (Q9: reseed, not requeue). Record the cause so
                                // the next pass's full walk labels itself
                                // `sink_fail`, not `coldboot`.
                                self.seeded.store(false, Ordering::SeqCst);
                                *self.pending_reseed_reason.lock().unwrap() =
                                    Some(FullReason::SinkFail);
                                return Err(e);
                            }
                        }
                    }
                } else {
                    full_reason = Some(FullReason::Unsettled);
                    tracing::warn!(
                        "[LoroProjection] incremental pass unsettled; reseeding from full snapshot"
                    );
                }
            } else {
                // Not a bounded incremental batch: empty queue with a moved
                // frontier, or an oversized batch (cold org-scan / bulk import).
                full_reason = Some(if pending.is_empty() {
                    FullReason::EmptyPendingMovedFrontier
                } else {
                    FullReason::Oversized
                });
            }
            // Not a bounded incremental batch (or unsettled) → drop through to
            // the full reseed path below, which reads current state (no
            // checkout).
        }

        // ── Full walk — cold-boot seed / reseed-on-unsettled / bootstrap ONLY ──
        // "after" = the full Loro authority snapshot. "before" = the ACTUAL SQL
        // sink state (read fresh), NOT the in-memory `live` base. This path is the
        // recovery/reseed path; it must reconcile the sink against Loro from
        // GROUND TRUTH. Diffing against `live` here was a latent silent-drift trap:
        // a reseed emits `diff(before, after)` but then sets `live = after`, so if
        // `live` ever diverged from `block_raw` (e.g. `live` holding a parent whose
        // sink row is absent), a `live`-based reseed would re-emit the delta
        // relative to the drifted base — never re-creating the missing parent —
        // while its descendants' creates FK-reject at COMMIT, wedging the projector
        // in a permanent error/reseed storm (keystone forward-edge/BulkExternalAdd
        // deferred-FK RED). Reading the sink makes every reseed converge
        // `block_raw` → Loro and re-establishes `live == block_raw` on success.
        // This path is not steady-state (cold boot / unsettled / orphan / oversized
        // bootstrap), so the extra sink read is not on the hot path.
        let (after, after_settled): (HashMap<String, SnapshotBlock>, bool) = {
            let doc = &*doc_arc;
            snapshot_blocks_from_doc_settled(doc)
        };
        let before: Arc<HashMap<String, SnapshotBlock>> = Arc::new(self.read_sql_snapshot().await?);
        let snapshot_ms = t0.elapsed().as_millis();

        let mut ops = diff_snapshots_to_ops(&before, &after);

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
        // Orphan-create gate (UNCONDITIONAL, TRANSITIVE). A create whose parent
        // `block_raw` row will not exist at COMMIT trips the deferred `parent_id`
        // self-FK and rolls the whole batch back — losing every co-batched row.
        // The parent is missing at COMMIT when it is neither in the sink base
        // (`before`) nor emitted by this batch. Two ways this arises:
        //   * a torn (unsettled) walk withheld the parent from `after`; or
        //   * a settled walk legitimately projects a child whose parent was dropped
        //     from the sink base earlier and is not in this snapshot (the incremental
        //     fast path funnels such a case here on purpose).
        // Crucially the check is TRANSITIVE: withholding an orphan parent must
        // also withhold its descendants, or a grandchild whose parent is present
        // in `after` (but itself withheld) FK-fails. `retain_grounded_creates`
        // grounds each create in the sink + the closure of grounded creates.
        {
            let withheld = retain_grounded_creates(&mut ops, &before);
            if withheld > 0 {
                tracing::warn!(
                    "[LoroProjection] withholding {} ungrounded orphan create(s) whose parent_id \
                     chain does not reach the sink base (snapshot_settled={})",
                    withheld,
                    after_settled,
                );
            }
        }

        let before_len = before.len();
        let after_len = after.len();

        // Every path that reaches here set `full_reason`; `None` = logic error.
        let full_reason = full_reason
            .expect("full reseed walk reached without a reason set")
            .as_str();

        // Apply to the sink FIRST; commit the new base state only AFTER the write
        // succeeds, so a failed apply (batch rollback) never advances `live` /
        // `last_synced` ahead of the sink (silent drift). On failure the pass
        // returns `Err` with the base untouched and retries next wake.
        if let Err(e) = self
            .emit_ops(
                ops,
                current,
                &t0,
                snapshot_ms,
                after_len,
                before_len,
                "full",
                full_reason,
            )
            .await
        {
            // Symmetric with the incremental path's Err arm: record `sink_fail`
            // so a retry pass (which sees only `seeded == false`) is labeled by
            // its true cause — a sink write failure — not mislabeled `coldboot`.
            *self.pending_reseed_reason.lock().unwrap() = Some(FullReason::SinkFail);
            return Err(e);
        }

        // Seed / refresh the incremental state — only on a settled snapshot — so
        // the next pass can take the fast path (and so a later reseed diffs against
        // `live`).
        if after_settled {
            let idx = {
                let doc = &*doc_arc;
                crate::loro_backend::build_tid_index(doc)
            };
            *self.tid_index.lock().unwrap() = idx;
            *self.live.lock().unwrap() = after;
            self.seeded.store(true, Ordering::SeqCst);
            // This full snapshot captured everything up to `current`, so any facts
            // accumulated before/during it are now stale — drop them so the next
            // incremental pass starts clean (bounds the queue during the pre-arm
            // boot window and after every reseed).
            self.pending.lock().unwrap().clear();
        }

        Ok(())
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
        reason: &str,
    ) -> Result<()> {
        // FK-safe create ordering — the single write chokepoint's guarantee.
        // The `block_raw.parent_id` self-FK is DEFERRABLE INITIALLY DEFERRED, but
        // the Turso fork only *decrements* the deferred-FK counter on a parent-key
        // UPDATE probe, never on a plain parent-row INSERT (see the fork's
        // translate/fkeys.rs — both `emit_parent_key_change_probes` callers are
        // UPDATE-path). So inserting a child row before its parent's INSERT
        // increments the counter with no matching decrement, and the batch's
        // COMMIT trips `deferred foreign key constraint failed on commit` even
        // though the final row set is FK-consistent. The full reseed path already
        // topo-sorts its creates (`diff_snapshots_to_ops`); the incremental fast
        // path emitted `changed` in arbitrary HashMap order — the residual
        // keystone deferred-FK RED. Ordering every batch parent-before-child here
        // makes both paths safe regardless of how the op vec was built.
        let ops = fk_order_creates_parent_first(ops);
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
                "[LoroSyncController OUTBOUND] mode={} after={} before={} ops={} \
                 aggregate_ids={:?}",
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
                "[LoroProjection] applied {} op(s) in {}ms (snapshot {}ms, after={} before={}) \
                 [{}]",
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
                reason = reason,
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
                    "[LoroSyncController] Sidecar at {} exists but is corrupt ({}); starting with \
                     empty watermark.",
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
        // FK dependencies that must exist as `block_raw` rows before this
        // block's create (row + junction writes) lands in the same batch:
        // the parent (`block_raw.parent_id`) AND every block-referencing edge
        // target (`block_requires.required_id`, `advice_suppressed.lesson_id`).
        // Ordering by parent alone loses BOTH blocks of a requires pair when
        // HashMap iteration puts the dependent first: the junction insert
        // FK-rejects and the whole batch transaction rolls back.
        let deps = std::iter::once(snap.block.parent_id.as_str())
            .chain(snap.block.requires.iter().map(|r| r.as_str()))
            .chain(snap.block.advice_suppressed.iter().map(|a| a.as_str()));
        for dep in deps {
            if create_ids.contains(dep)
                && let Some(dep_snap) = all.get(dep)
            {
                visit(dep_snap, all, create_ids, visited, result);
            }
        }
        result.push(snap);
    }

    for snap in &creates {
        visit(snap, all, &create_ids, &mut visited, &mut result);
    }
    result
}

/// Reorder a batch so every `create` precedes any co-batched `create` of one of
/// its `block_raw.parent_id` ancestors (parent-before-child), keeping all
/// non-create ops in their original relative order after the creates.
///
/// Why this is load-bearing: the `parent_id` self-FK is DEFERRABLE INITIALLY
/// DEFERRED, so a child inserted before its parent is legal *by SQL semantics*
/// — the check defers to COMMIT. But the Turso fork only decrements the
/// deferred-FK violation counter on a parent-key UPDATE probe, never on a plain
/// parent-row INSERT, so a child-before-parent INSERT increments the counter
/// with no matching decrement and the COMMIT fails even though the final rows
/// are consistent. The full reseed path sidesteps this by topo-sorting its
/// creates (`topological_sort_creates`); the incremental fast path built ops in
/// `HashMap` order. Applying this at the `emit_ops` chokepoint makes EVERY
/// batch FK-safe, independent of the builder.
///
/// Only intra-batch parent edges are ordered — a parent already committed in
/// the sink is a valid FK target at any position. A malformed create (no `id`)
/// keeps its arrival position and is left for the sink to reject loudly.
fn fk_order_creates_parent_first(
    ops: Vec<(String, holon_api::StorageEntity)>,
) -> Vec<(String, holon_api::StorageEntity)> {
    let mut creates: Vec<(String, holon_api::StorageEntity)> = Vec::new();
    let mut tail: Vec<(String, holon_api::StorageEntity)> = Vec::new();
    for op in ops {
        if op.0 == "create" {
            creates.push(op);
        } else {
            tail.push(op);
        }
    }

    let mut idx_of: HashMap<String, usize> = HashMap::new();
    for (i, (_, e)) in creates.iter().enumerate() {
        if let Some(id) = e.get("id").and_then(|v| v.as_string()) {
            idx_of.insert(id.to_string(), i);
        }
    }

    fn visit(
        i: usize,
        creates: &[(String, holon_api::StorageEntity)],
        idx_of: &HashMap<String, usize>,
        visited: &mut [bool],
        order: &mut Vec<usize>,
    ) {
        if visited[i] {
            return;
        }
        visited[i] = true;
        if let Some(pid) = creates[i].1.get("parent_id").and_then(|v| v.as_string())
            && let Some(&pi) = idx_of.get(pid)
            && pi != i
        {
            visit(pi, creates, idx_of, visited, order);
        }
        order.push(i);
    }

    let n = creates.len();
    let mut visited = vec![false; n];
    let mut order: Vec<usize> = Vec::with_capacity(n);
    for i in 0..n {
        visit(i, &creates, &idx_of, &mut visited, &mut order);
    }

    let mut slots: Vec<Option<(String, holon_api::StorageEntity)>> =
        creates.into_iter().map(Some).collect();
    let mut result: Vec<(String, holon_api::StorageEntity)> = Vec::with_capacity(n + tail.len());
    for i in order {
        result.push(slots[i].take().expect("each create visited exactly once"));
    }
    result.extend(tail);
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

#[cfg(test)]
mod fk_order_tests {
    use super::*;

    fn create(id: &str, parent: &str) -> (String, holon_api::StorageEntity) {
        let mut e = holon_api::StorageEntity::new();
        e.insert("id".into(), Value::String(id.into()));
        e.insert("parent_id".into(), Value::String(parent.into()));
        ("create".to_string(), e)
    }
    fn other(kind: &str, id: &str) -> (String, holon_api::StorageEntity) {
        let mut e = holon_api::StorageEntity::new();
        e.insert("id".into(), Value::String(id.into()));
        (kind.to_string(), e)
    }
    fn ids(ops: &[(String, holon_api::StorageEntity)]) -> Vec<String> {
        ops.iter()
            .map(|(n, e)| format!("{n}:{}", e.get("id").unwrap().as_string().unwrap()))
            .collect()
    }
    fn pos(ordered: &[(String, holon_api::StorageEntity)], id: &str) -> usize {
        ordered
            .iter()
            .position(|(_, e)| e.get("id").unwrap().as_string() == Some(id))
            .unwrap()
    }

    /// A child emitted before its intra-batch parent (the exact keystone
    /// deferred-FK shape: `create c<-p` at index 0, `create p<-sink` at index
    /// 1) must be reordered parent-before-child.
    #[test]
    fn child_before_parent_is_reordered() {
        let ops = vec![
            create("block:c", "block:p"),
            create("block:p", "block:sink-root"),
        ];
        let out = fk_order_creates_parent_first(ops);
        assert!(
            pos(&out, "block:p") < pos(&out, "block:c"),
            "{:?}",
            ids(&out)
        );
    }

    /// Transitive chain a<-b<-c fed in reverse still lands root-first.
    #[test]
    fn transitive_chain_ordered_root_first() {
        let ops = vec![
            create("block:c", "block:b"),
            create("block:b", "block:a"),
            create("block:a", "block:sink"),
        ];
        let out = fk_order_creates_parent_first(ops);
        assert!(pos(&out, "block:a") < pos(&out, "block:b"));
        assert!(pos(&out, "block:b") < pos(&out, "block:c"));
    }

    /// Non-create ops keep their relative order and follow all creates.
    #[test]
    fn non_creates_kept_after_creates_in_order() {
        let ops = vec![
            other("update", "block:u1"),
            create("block:c", "block:p"),
            create("block:p", "block:sink"),
            other("delete", "block:d1"),
            other("update", "block:u2"),
        ];
        let out = fk_order_creates_parent_first(ops);
        let labels = ids(&out);
        // creates first (both), then the tail in original order.
        assert_eq!(
            &labels[2..],
            &["update:block:u1", "delete:block:d1", "update:block:u2"]
        );
        assert!(pos(&out, "block:p") < pos(&out, "block:c"));
    }

    /// A parent already in the sink (not co-batched) imposes no ordering; the
    /// batch is returned effectively as-is (creates stable).
    #[test]
    fn sink_parent_needs_no_reorder() {
        let ops = vec![
            create("block:a", "block:sink"),
            create("block:b", "block:sink"),
        ];
        let out = fk_order_creates_parent_first(ops);
        assert_eq!(ids(&out), vec!["create:block:a", "create:block:b"]);
    }

    /// A self-parent cycle must not infinite-loop.
    #[test]
    fn self_parent_terminates() {
        let ops = vec![create("block:x", "block:x")];
        let out = fk_order_creates_parent_first(ops);
        assert_eq!(ids(&out), vec!["create:block:x"]);
    }
}

/// Withhold "orphan" creates whose `parent_id` chain does not bottom out in the
/// sink base (`before`) or a non-`block:` sentinel, returning the count
/// removed.
///
/// A create's `block_raw.parent_id` self-FK is DEFERRABLE INITIALLY DEFERRED,
/// so it is checked at COMMIT. Emitting a create whose parent row is neither
/// pre-existing in the sink nor inserted by this same batch trips that FK and
/// rolls back the WHOLE batch — losing every co-batched row (the
/// forward-edge/BulkExternalAdd deferred-FK RED).
///
/// The check MUST be a fixpoint over the surviving-create set, not a
/// single-level "is the parent somewhere in the projected snapshot" lookup. A
/// parent's mere presence in the Loro snapshot (`after`) does NOT mean a row
/// will be inserted for it: a torn/unsettled walk may have withheld the parent,
/// or the parent may itself be a transitively-orphaned create that this gate
/// withholds. In both cases the parent is in `after` yet never lands in
/// `block_raw`, so a child admitted merely because `after.contains(parent)`
/// FK-fails at COMMIT. Ground each create in the SINK plus the transitive
/// closure of grounded creates instead: `before` rows and non-`block:`
/// sentinels are the axioms; a create is grounded once its parent is grounded;
/// anything left ungrounded is withheld (re-emitted on a later pass once its
/// parent actually reaches the sink).
fn retain_grounded_creates(
    ops: &mut Vec<(String, holon_api::StorageEntity)>,
    before: &HashMap<String, SnapshotBlock>,
) -> usize {
    // id -> parent_id for every create in the batch. A create with no `id` param
    // is malformed; it is left in `ops` (grounded-by-default below) so the sink
    // fails loudly on it rather than this gate hiding it.
    let create_parents: Vec<(String, String)> = ops
        .iter()
        .filter(|(name, _)| name == "create")
        .filter_map(|(_, e)| {
            let id = e.get("id").and_then(|v| v.as_string())?;
            let pid = e.get("parent_id").and_then(|v| v.as_string()).unwrap_or("");
            Some((id.to_string(), pid.to_string()))
        })
        .collect();

    // A non-`block:` parent (seed layout / `sentinel` root) is always satisfiable;
    // a parent already in the sink base is always a valid FK target.
    let parent_satisfiable = |grounded: &std::collections::HashSet<String>, pid: &str| {
        !pid.starts_with("block:") || before.contains_key(pid) || grounded.contains(pid)
    };

    // Fixpoint: admit any create whose parent is grounded, until none remain.
    let mut grounded: std::collections::HashSet<String> = std::collections::HashSet::new();
    loop {
        let mut added = false;
        for (id, pid) in &create_parents {
            if !grounded.contains(id) && parent_satisfiable(&grounded, pid) {
                grounded.insert(id.clone());
                added = true;
            }
        }
        if !added {
            break;
        }
    }

    let n = ops.len();
    ops.retain(|(name, e)| {
        if name != "create" {
            return true;
        }
        match e.get("id").and_then(|v| v.as_string()) {
            Some(id) => grounded.contains(id),
            None => true,
        }
    });
    n - ops.len()
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
    // `collapsed` is a typed Block field: `read_block_from_tree` lifts it out
    // of the Loro properties map, so the properties diff below can never see
    // it — compare it explicitly here (in lockstep with `blocks_differ`'s
    // plain `!=` over the whole Block).
    if old.collapsed != new.collapsed {
        params.insert("collapsed".into(), Value::Boolean(new.collapsed));
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

    // DIAGNOSTIC (intent-divergence hunt): a params map carrying ONLY the
    // bookkeeping keys means `blocks_differ` said the pair differs but no
    // representable field was emitted — the update decodes to zero typed ops
    // and trips the consolidator's `agrees_with_ops` divergence. Dump the two
    // blocks field-by-field so the asymmetric field is unambiguous.
    // `id` + `updated_at` are always inserted; len == 2 ⟺ no representable field.
    if params.len() == 2 {
        tracing::error!(
            "[block_diff_params] BOOKKEEPING-ONLY update for {} — blocks_differ true but no field \
             emitted. sort_key: {:?} vs {:?} | content: {:?} vs {:?} | parent: {:?} vs {:?} | \
             content_type: {:?} vs {:?} | source_language: {:?} vs {:?} | source_name: {:?} vs \
             {:?} | tags: {:?} vs {:?} | requires: {:?} vs {:?} | advice_suppressed: {:?} vs {:?} \
             | collapsed: {} vs {} | properties: {:?} vs {:?} | marks: {:?} vs {:?}",
            new.id,
            old_sort_key,
            new_sort_key,
            old.content,
            new.content,
            old.parent_id,
            new.parent_id,
            old.content_type,
            new.content_type,
            old.source_language,
            new.source_language,
            old.source_name,
            new.source_name,
            old.tags,
            new.tags,
            old.requires,
            new.requires,
            old.advice_suppressed,
            new.advice_suppressed,
            old.collapsed,
            new.collapsed,
            old.properties,
            new.properties,
            old.marks,
            new.marks,
        );
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

    use holon_api::EntityUri;
    use holon_api::InlineMark;
    use holon_api::MarkSpan;

    use super::*;

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
    /// `source_language` (`Some → None`) was detected by `blocks_differ` but
    /// the old `let Some(new) = …` guard emitted NO param, so the clear
    /// never reached SQL and the update decoded to zero typed ops. Must now
    /// emit `Null` (mirror `marks`), keeping the diff in lockstep with
    /// `blocks_differ`.
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
            "cleared source_language must emit a Null sentinel (else the clear is dropped from \
             SQL and the update decodes to zero typed ops): {params:?}"
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
    /// the stale value lived in SQL forever (mirror of the source_language
    /// fix).
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
            "removed property must emit a Null sentinel (else SQL keeps the stale value forever): \
             {params:?}"
        );
    }

    /// The removal update must also round-trip through the typed intent
    /// vocabulary (no `agrees_with_ops` divergence): the Null decodes to a
    /// SetField, so source `update:1` re-encodes to `update:1`.
    #[test]
    fn property_removal_update_agrees_with_ops() {
        use holon_api::ChangeSet;
        use holon_api::Provenance;
        use holon_api::agrees_with_ops;
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
        use holon_api::ChangeSet;
        use holon_api::Provenance;
        use holon_api::agrees_with_ops;
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
            !params.contains_key("_expected_marks"), /* ALLOW(deleted_cell_symbol): test asserts
                                                      * the watermark name is ABSENT
                                                      * post-authority-flip */
            "watermark dropped post-authority-flip; got params: {params:?}"
        );
        let new_val = params.get("marks").expect("new marks present");
        let s = new_val.as_string().expect("marks is String");
        let parsed: Vec<MarkSpan> = holon_api::marks_from_json(s).expect("parse new");
        assert_eq!(parsed, new_marks);
    }
}

#[cfg(test)]
mod orphan_gate_tests {
    //! Transitive orphan-create gate (`retain_grounded_creates`).
    //!
    //! Regression for the forward-edge / BulkExternalAdd deferred-FK RED: a
    //! projection batch that emits a child create while WITHHOLDING its parent
    //! create trips the deferred `block_raw.parent_id` self-FK at COMMIT and
    //! rolls the whole batch back. The old single-level gate (parent present in
    //! `after` OR `before`) admitted such descendants because the withheld
    //! parent was still present in the projected snapshot. Grounding must be
    //! transitive: a create survives only if its parent chain reaches the sink.

    use holon_api::EntityUri;

    use super::*;

    fn create_op(id: &str, parent: &str) -> (String, holon_api::StorageEntity) {
        let mut e = holon_api::StorageEntity::new();
        e.insert("id".into(), Value::String(id.to_string()));
        e.insert("parent_id".into(), Value::String(parent.to_string()));
        ("create".to_string(), e)
    }

    fn sink(ids: &[&str]) -> HashMap<String, SnapshotBlock> {
        ids.iter()
            .map(|id| {
                let block = Block::new_text(
                    EntityUri::block(id.trim_start_matches("block:")),
                    EntityUri::no_parent(),
                    String::new(),
                );
                (
                    id.to_string(),
                    SnapshotBlock {
                        block,
                        sort_key: "A0".to_string(),
                    },
                )
            })
            .collect()
    }

    fn kept_ids(ops: &[(String, holon_api::StorageEntity)]) -> Vec<String> {
        ops.iter()
            .filter(|(n, _)| n == "create")
            .map(|(_, e)| e.get("id").and_then(|v| v.as_string()).unwrap().to_string())
            .collect()
    }

    /// The shrunk keystone topology. `forward-edge-page` and `block:bulk-6-3`
    /// are the two roots; when they are ABSENT from the sink base (torn
    /// walk withheld them / not yet projected), their whole subtrees must
    /// be withheld — not just the roots. The old gate kept `bulk-6-1`
    /// (parent `forward-edge-page` ∈ `after`) and `bulk-6-4`/`bulk-6-8`
    /// (parent `bulk-6-3` ∈ `after`), FK-failing.
    #[test]
    fn shrunk_topology_withholds_whole_subtree_when_roots_absent() {
        let mut ops = vec![
            create_op("block:bulk-6-1", "block:forward-edge-page"),
            create_op("block:bulk-6-6", "block:forward-edge-page"),
            create_op("block:bulk-6-7", "block:bulk-6-1"),
            create_op("block:bulk-6-5", "block:bulk-6-4"),
            create_op("block:bulk-6-4", "block:bulk-6-3"),
            create_op("block:bulk-6-8", "block:bulk-6-3"),
        ];
        // Neither root is in the sink and neither is a create in this batch.
        let before = sink(&[]);
        let withheld = retain_grounded_creates(&mut ops, &before);
        assert_eq!(
            withheld,
            6,
            "every create is ungrounded; kept: {:?}",
            kept_ids(&ops)
        );
        assert!(kept_ids(&ops).is_empty());
    }

    /// Same topology, but both roots ARE in the sink base: every create is
    /// grounded (parent chain reaches `before`) and nothing is withheld.
    #[test]
    fn shrunk_topology_admits_all_when_roots_in_sink() {
        let mut ops = vec![
            create_op("block:bulk-6-1", "block:forward-edge-page"),
            create_op("block:bulk-6-6", "block:forward-edge-page"),
            create_op("block:bulk-6-7", "block:bulk-6-1"),
            create_op("block:bulk-6-5", "block:bulk-6-4"),
            create_op("block:bulk-6-4", "block:bulk-6-3"),
            create_op("block:bulk-6-8", "block:bulk-6-3"),
        ];
        let before = sink(&["block:forward-edge-page", "block:bulk-6-3"]);
        let withheld = retain_grounded_creates(&mut ops, &before);
        assert_eq!(withheld, 0, "all grounded; withheld {withheld}");
        assert_eq!(kept_ids(&ops).len(), 6);
    }

    /// Chain of 3 (grandparent → parent → child) with the grandparent WITHHELD:
    /// a single-level gate keeps `parent` (its parent `gp` ∈ `after`) and
    /// `child` (its parent `parent` ∈ `after`), both FK-doomed. Transitive
    /// grounding withholds the entire chain when `gp` is absent.
    #[test]
    fn chain_of_three_absent_grandparent_withholds_all() {
        let mut ops = vec![
            create_op("block:child", "block:parent"),
            create_op("block:parent", "block:gp"),
            // gp is NOT a create here and NOT in the sink → ungrounded root.
        ];
        let before = sink(&[]);
        let withheld = retain_grounded_creates(&mut ops, &before);
        assert_eq!(withheld, 2, "kept: {:?}", kept_ids(&ops));
        assert!(kept_ids(&ops).is_empty());
    }

    /// Chain of 3 fully in-batch with the grandparent GROUNDED in the sink: the
    /// batch order is deliberately child-first, proving grounding is
    /// order-free.
    #[test]
    fn chain_of_three_grounded_grandparent_admits_all_regardless_of_order() {
        let mut ops = vec![
            create_op("block:child", "block:parent"),
            create_op("block:parent", "block:gp"),
        ];
        let before = sink(&["block:gp"]);
        let withheld = retain_grounded_creates(&mut ops, &before);
        assert_eq!(withheld, 0);
        assert_eq!(kept_ids(&ops).len(), 2);
    }

    /// A non-`block:` sentinel parent (seed layout root) is always satisfiable.
    #[test]
    fn sentinel_parent_is_always_grounded() {
        let mut ops = vec![
            create_op("block:root", "layout:main"),
            create_op("block:leaf", "block:root"),
        ];
        let before = sink(&[]);
        let withheld = retain_grounded_creates(&mut ops, &before);
        assert_eq!(withheld, 0, "sentinel-rooted subtree must survive");
        assert_eq!(kept_ids(&ops).len(), 2);
    }

    /// Updates and deletes flow untouched; only ungrounded creates are
    /// withheld.
    #[test]
    fn non_creates_pass_through() {
        let mut update = holon_api::StorageEntity::new();
        update.insert("id".into(), Value::String("block:u".into()));
        let mut delete = holon_api::StorageEntity::new();
        delete.insert("id".into(), Value::String("block:d".into()));
        let mut ops = vec![
            ("update".to_string(), update),
            ("delete".to_string(), delete),
            create_op("block:orphan", "block:missing"),
        ];
        let before = sink(&[]);
        let withheld = retain_grounded_creates(&mut ops, &before);
        assert_eq!(withheld, 1);
        assert_eq!(ops.len(), 2);
        assert!(ops.iter().any(|(n, _)| n == "update"));
        assert!(ops.iter().any(|(n, _)| n == "delete"));
    }
}

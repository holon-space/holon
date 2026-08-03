//! Operations for sharing and mounting Loro subtrees across peers.
//!
//! Registered on entity `"tree"`. Two operations:
//! - `share_subtree(id, retention)` → returns a base64 ticket in `response`
//! - `accept_shared_subtree(parent_id, ticket)` → returns the new mount block's
//!   stable id in `response`
//!
//! See the crate-level plan in docs/Reference/SUBTREE_SHARING.md for the threat
//! model.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::DownstreamProjection;
use holon_core::MaybeSendSync;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::OriginTaggedWrites;
use holon_core::Result;
use holon_core::UndoAction;
use iroh::EndpointAddr;
use iroh::SecretKey;
use loro::LoroDoc;
use loro::TreeID;
use loro::TreeParentId;
use loro::ValueOrContainer;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tokio::time::timeout;
use tracing::warn;
use uuid::Uuid;

use crate::debounced_commit_worker::DebouncedCommitWorkerHandle;
use crate::debounced_commit_worker::any_commit;
use crate::debounced_commit_worker::local_only;
use crate::debounced_commit_worker::{self};
use crate::degraded_signal_bus::DegradedSignalBus;
use crate::degraded_signal_bus::ShareDegraded;
use crate::degraded_signal_bus::ShareDegradedReason;
use crate::iroh_advertiser::ALPN_PREFIX;
use crate::iroh_advertiser::IrohAdvertiser;
use crate::iroh_advertiser::OnPeerConnected;
use crate::iroh_sync_adapter::SharedTreeSyncManager;
use crate::iroh_sync_adapter::create_endpoint;
use crate::iroh_sync_adapter::make_alpn;
use crate::iroh_sync_adapter::sync_doc_initiate;
use crate::loro_document_store::LoroDocumentStore;
use crate::loro_sync_controller::project_shared_doc_to_ops;
use crate::share_peer_id::stable_peer_id;
use crate::shared_snapshot_store::SharedSnapshotStore;
use crate::shared_tree::HistoryRetention;
use crate::shared_tree::SHARE_ROLE_MOUNT;
use crate::shared_tree::SHARE_ROLE_PROPERTY;
use crate::shared_tree::SHARED_TREE_ID_PROPERTY;
use crate::shared_tree::{self};
use crate::ticket::Ticket;

fn err(msg: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::<dyn std::error::Error + Send + Sync>::from(msg.into())
}

/// Entity name under which `LoroShareBackend` registers its operations.
///
/// Picked as a single bare word with no `_` so it survives
/// [`EntityName::new`]'s URI-scheme normalization (`_` → `-`) unchanged.
/// This avoids the hyphen/underscore mismatch that would otherwise bite
/// every `entity_name == TREE_ENTITY` comparison against an already-
/// normalized `EntityName`.
pub const TREE_ENTITY: &str = "tree";
use crate::loro_backend::STABLE_ID;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default enrollment window for a share ticket's capability: after this many
/// seconds no *new* peer may enroll (already-enrolled peers keep syncing). 30
/// days is a placeholder pending the lease-policy ruling (ADR 0028 D4/H8).
const DEFAULT_ENROLLMENT_WINDOW_SECS: i64 = 30 * 24 * 60 * 60;

/// Stable id (bare, no `block:` prefix) of the recipient-side **"Shared with
/// me" root** — the dedicated home for accepted shares (ADR 0028 H7). A single
/// well-known id makes the root idempotent to ensure: every accept resolves the
/// same node instead of minting a fresh orphan. Accepted mounts that would
/// otherwise bubble to `no_parent` (an invisible top-level orphan — dogfood N3,
/// 2026-07-20) attach here instead, so the shared content is reachable and
/// rendered under a visible page in the UI.
pub const SHARED_WITH_ME_ROOT_ID: &str = "shared-with-me";
/// Display title of the "Shared with me" recipient root.
pub const SHARED_WITH_ME_TITLE: &str = "Shared with me";

/// Operations for creating and accepting shared Loro subtrees.
#[holon_macros::operations_trait]
#[async_trait]
pub trait SubtreeShareOperations<T>: MaybeSendSync
where
    T: MaybeSendSync + 'static,
{
    /// Share the subtree rooted at `id`. Returns a base64-encoded ticket
    /// via `OperationResult::response`.
    ///
    /// `retention` must be `"full"` or `"none"`.
    #[holon_macros::affects("parent_id")]
    async fn share_subtree(&self, id: &str, retention: String) -> Result<OperationResult>;

    /// Accept a shared subtree under `parent_id`, using a ticket generated
    /// by `share_subtree` on the other peer. Returns the new mount block's
    /// stable id via `OperationResult::response`.
    #[holon_macros::affects("parent_id")]
    async fn accept_shared_subtree(
        &self,
        parent_id: &str,
        ticket: String,
    ) -> Result<OperationResult>;

    /// Delete orphan snapshot files on disk — `shares/<id>.loro` (+ its
    /// `.peers.json` sidecar and any `.corrupt-*` siblings) for which
    /// no mount node exists in the global tree. User-driven only: the
    /// op runs when invoked. The UI is responsible for any
    /// "are you sure" gate. Returns the deleted `shared_tree_id`s in
    /// the response JSON under `deleted`.
    #[holon_macros::affects("parent_id")]
    async fn gc_orphans(&self) -> Result<OperationResult>;

    /// Stop sharing the subtree whose mount block is `mount_block_id`.
    ///
    /// Tears the share down in a resurrection-safe order: drop the per-share
    /// workers FIRST (so no worker can re-write the snapshot after we delete
    /// it), close the advertiser endpoint, unregister the shared doc, delete
    /// the mount node from the global tree (so the block disappears from the
    /// UI) plus its projected SQL rows, and finally delete the on-disk
    /// snapshot. Returns the removed `shared_tree_id` + `mount_block_id` in
    /// the response JSON. Loud error if `mount_block_id` is not a mount.
    #[holon_macros::affects("parent_id")]
    async fn unshare(&self, mount_block_id: &str) -> Result<OperationResult>;
}

/// Holder for a per-share save worker. Wraps the generic
/// [`DebouncedCommitWorkerHandle`] plus the `Arc<LoroDoc>` the worker
/// persists — `flush_all` needs the `doc` handle to force a final save
/// before shutdown without waiting for the debounce window.
struct SaveWorker {
    _handle: DebouncedCommitWorkerHandle,
    doc: Arc<LoroDoc>,
}

/// How long to coalesce burst commits before writing to disk. Small
/// enough that a `SIGKILL` loses only a few keystrokes; large enough
/// that a typing burst produces one write, not hundreds.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(150);

fn spawn_save_worker(
    store: Arc<SharedSnapshotStore>,
    bus: Arc<DegradedSignalBus>,
    shared_tree_id: String,
    doc: Arc<LoroDoc>,
) -> SaveWorker {
    let id_for_work = shared_tree_id.clone();
    let doc_for_work = doc.clone();
    let store_for_work = store.clone();
    let bus_for_work = bus.clone();
    let handle = debounced_commit_worker::spawn(
        doc.clone(),
        any_commit(),
        SAVE_DEBOUNCE,
        "share.save",
        move || {
            let id = id_for_work.clone();
            let doc = doc_for_work.clone();
            let store = store_for_work.clone();
            let bus = bus_for_work.clone();
            async move {
                if let Err(e) = store.save(&id, &doc) {
                    // Emit the degraded-mode signal for the UI. The
                    // `Err` return also surfaces in the worker's
                    // tracing::error so both the bus-listener and the
                    // operator log the same failure — no swallowing.
                    bus.emit(ShareDegraded {
                        shared_tree_id: id.clone(),
                        reason: ShareDegradedReason::SnapshotSaveFailed(format!("{e:#}")),
                    });
                    return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                        "snapshot save for {id} failed: {e:#}"
                    )));
                }
                Ok(())
            }
        },
    );

    SaveWorker {
        _handle: handle,
        doc,
    }
}

/// Backing state for subtree share operations. Kept separate from
/// `LoroBackend` (which has many other responsibilities) so the iroh
/// endpoint lifecycle is isolated.
pub struct LoroShareBackend {
    store: Arc<RwLock<LoroDocumentStore>>,
    snapshot_store: Arc<SharedSnapshotStore>,
    manager: Arc<SharedTreeSyncManager>,
    advertiser: Arc<IrohAdvertiser>,
    degraded_bus: Arc<DegradedSignalBus>,
    device_key: SecretKey,
    /// Handle to the SQL `block` table. Used to project mount nodes into
    /// Block rows after `accept_shared_subtree` / `rehydrate_shared_trees`
    /// so the UI — which reads from SQL — can render shared content.
    /// `Option` so tests that construct the backend directly (without the
    /// full DI stack) can keep working; when `None`, mount-node projection
    /// is skipped.
    sql_ops: Option<Arc<dyn OriginTaggedWrites>>,
    /// The global Loro→SQL projection (the same `DownstreamProjection`
    /// `LoroSyncController` drives). `share_subtree` flushes it after
    /// pruning the shared subtree from the global tree so the global
    /// prune-delete diff is applied to SQL — and its watermark advances —
    /// BEFORE the sharer re-projects the subtree under the mount. Without
    /// this barrier the global delete-diff races the re-projection and can
    /// re-remove the just-re-created rows (see `share_subtree`). `Option`
    /// for the same reason as `sql_ops`: tests without the DI stack skip it.
    downstream_projection: Option<Arc<dyn DownstreamProjection>>,
    /// `shared_tree_id → known peer endpoint addrs`. Populated on
    /// accept (ticket author's addr), on every inbound advertiser
    /// handshake, and at startup from the sidecar JSON.
    known_peers: Arc<RwLock<HashMap<String, Vec<EndpointAddr>>>>,
    /// Per-share save worker. Dropped when `unregister` is called or
    /// when the backend itself is dropped.
    save_workers: Arc<RwLock<HashMap<String, SaveWorker>>>,
    /// Per-share auto-resync worker. Fires `sync_with_peers` on local
    /// commits (debounced at `SYNC_DEBOUNCE`, separate from the save
    /// debounce because the cadence goals differ).
    sync_workers: Arc<RwLock<HashMap<String, SyncWorker>>>,
    /// Per-share SQL projection worker. On each shared-doc change
    /// (debounced), diffs the doc and writes creates/updates/deletes
    /// into the SQL block table so the UI stays in sync.
    projection_workers: Arc<RwLock<HashMap<String, ProjectionWorker>>>,
    /// Weak reference to self, populated during `Arc::new_cyclic`
    /// construction — the closure receives a `&Weak<Self>` BEFORE the
    /// `Arc` is fully built, so the field is set up once atomically.
    /// Internal callbacks (e.g. the advertiser's on-peer-connected
    /// hook, the per-share auto-resync worker) upgrade this weak ref
    /// to call back into `&self`-shaped methods without threading
    /// `Arc<Self>` through every call site.
    self_weak: std::sync::Weak<LoroShareBackend>,
}

/// Holder for a per-share auto-resync worker. Wraps the generic
/// [`DebouncedCommitWorkerHandle`] — no per-worker state beyond the
/// handle itself (unlike `SaveWorker`, which keeps the `doc` handle
/// alive for `flush_all`).
struct SyncWorker {
    _handle: DebouncedCommitWorkerHandle,
}

/// Debounce for the auto-resync worker. Larger than `SAVE_DEBOUNCE`
/// because network round-trips cost more than a local disk write —
/// coalescing a typing burst into one sync saves the other peer from
/// a flood of tiny deltas.
const SYNC_DEBOUNCE: Duration = Duration::from_millis(500);

/// Spawn the auto-resync worker for a shared doc.
///
/// Uses the generic [`DebouncedCommitWorker`] with the `local_only()`
/// filter — `Import` events are remote updates just applied by the
/// sync protocol, and syncing them back out would churn forever. The
/// filter uses `EventTriggerKind::Local` (what Loro hands us); the
/// alternative of comparing the top-level change's peer id to
/// `stable_peer_id(device_key, shared_tree_id)` yields the same
/// outcome.
///
/// `local_peer_id` is accepted as a parameter purely so the tracing
/// line shows it for debugging — the filter itself is `local_only()`.
fn spawn_sync_worker(
    weak_backend: std::sync::Weak<LoroShareBackend>,
    shared_tree_id: String,
    doc: Arc<LoroDoc>,
    local_peer_id: u64,
) -> SyncWorker {
    let id_for_work = shared_tree_id;
    let handle =
        debounced_commit_worker::spawn(doc, local_only(), SYNC_DEBOUNCE, "share.sync", move || {
            let id = id_for_work.clone();
            let weak = weak_backend.clone();
            async move {
                let Some(backend) = weak.upgrade() else {
                    // Backend dropped — the next iteration will never
                    // fire because the worker handle is dropped with
                    // the backend. This branch is only reached if the
                    // callback is invoked concurrently with drop.
                    return Ok(());
                };
                let n = backend.sync_with_peers(&id).await.map_err(|e| {
                    Box::<dyn std::error::Error + Send + Sync>::from(format!(
                        "auto-resync for {id} failed: {e:#}"
                    ))
                })?;
                tracing::debug!(
                    shared_tree_id = %id,
                    local_peer_id = %local_peer_id,
                    peers_synced = %n,
                    "[share] auto-resync fired"
                );
                Ok(())
            }
        });

    SyncWorker { _handle: handle }
}

/// Per-share SQL projection worker. On every change to the shared doc
/// (debounced), diffs the current state against a frontiers watermark
/// and projects creates/updates/deletes into the SQL block table.
struct ProjectionWorker {
    _handle: DebouncedCommitWorkerHandle,
}

/// Debounce for the SQL projection worker. Same cadence as save —
/// every local or imported change should become visible in the UI
/// quickly, but not so quickly that a typing burst floods the DB.
const PROJECTION_DEBOUNCE: Duration = Duration::from_millis(150);

fn spawn_projection_worker(
    doc: Arc<LoroDoc>,
    sql_ops: Arc<dyn OriginTaggedWrites>,
    bus: Arc<DegradedSignalBus>,
    mount_block_uri: String,
    shared_tree_id: String,
    global_doc: Arc<crate::loro_document::LoroDocument>,
) -> ProjectionWorker {
    use std::sync::Mutex as StdMutex;

    use crate::loro_backend::snapshot_blocks_from_doc;
    use crate::loro_sync_controller::is_empty_frontiers;

    let watermark = Arc::new(StdMutex::new(doc.oplog_frontiers()));
    let mount_uri =
        EntityUri::parse(&mount_block_uri).expect("mount_block_uri must be a valid URI");
    let handle = debounced_commit_worker::spawn(
        doc.clone(),
        any_commit(),
        PROJECTION_DEBOUNCE,
        "share.project",
        move || {
            let doc = doc.clone();
            let sql_ops = sql_ops.clone();
            let bus = bus.clone();
            let mount_uri = mount_uri.clone();
            let stid = shared_tree_id.clone();
            let watermark = watermark.clone();
            let global_doc = global_doc.clone();
            async move {
                let current = doc.oplog_frontiers();
                let last = watermark.lock().unwrap().clone();
                if last == current {
                    return Ok(());
                }

                let patch = |blocks: &mut HashMap<String, crate::loro_backend::SnapshotBlock>| {
                    for snap in blocks.values_mut() {
                        let block = &mut snap.block;
                        if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                            block.parent_id = mount_uri.clone();
                        }
                        block
                            .properties
                            .entry(SHARED_TREE_ID_PROPERTY.to_string())
                            .or_insert_with(|| Value::String(stid.clone()));
                    }
                };

                let before = if is_empty_frontiers(&last) {
                    HashMap::new()
                } else {
                    let fork = doc.fork_at(&last).map_err(|e| {
                        Box::<dyn std::error::Error + Send + Sync>::from(format!(
                            "shared doc projection for {stid}: fork_at watermark failed: {e}"
                        ))
                    })?;
                    let mut snap = snapshot_blocks_from_doc(&fork);
                    patch(&mut snap);
                    snap
                };

                let (ops, after_settled) = share_diff_ops(&doc, &before, &patch, &stid);
                if !ops.is_empty() {
                    // Integrity guard (see `first_local_collision`): a synced-in
                    // remote edit must never project a block whose id shadows a
                    // LIVE local block. Refuse loudly (banner + `Err` freezes the
                    // watermark) instead of letting the remote clobber the
                    // recipient's own SQL row.
                    if let Some(bad) = global_doc
                        .with_read(|d| Ok(first_local_collision(d, &ops)))
                        .map_err(|e| {
                            Box::<dyn std::error::Error + Send + Sync>::from(format!(
                                "shared doc {stid}: global-doc collision-guard read failed: {e:#}"
                            ))
                        })?
                    {
                        bus.emit(ShareDegraded {
                            shared_tree_id: stid.clone(),
                            reason: ShareDegradedReason::ForeignIdCollision(bad.clone()),
                        });
                        return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                            "shared doc {stid} projection refused: block id {bad:?} collides with \
                             a live local block — refusing to clobber local content"
                        )));
                    }
                    let entity = EntityName::new("block");
                    if let Err(e) = sql_ops
                        .execute_batch_with_origin(
                            &entity,
                            ops,
                            crate::event_bus::EventOrigin::Loro,
                        )
                        .await
                    {
                        // Loro accepted the change but SQL rejected the
                        // projection — the two now diverge and the UI (which
                        // reads SQL) is stale. Surface a banner via the bus
                        // AND return `Err` so the watermark stays put and the
                        // next commit retries. Mirrors the save worker's
                        // fail-loud contract; the generic worker loop also
                        // logs the `Err`.
                        bus.emit(ShareDegraded {
                            shared_tree_id: stid.clone(),
                            reason: ShareDegradedReason::SqlProjectionFailed(format!("{e:#}")),
                        });
                        return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                            "shared doc projection for {stid} failed: {e:#}"
                        )));
                    }
                }

                if after_settled {
                    *watermark.lock().unwrap() = current;
                } else {
                    // Freezing the watermark keeps the pre-mutation base:
                    // withheld deletes (including LEGITIMATE ones that
                    // happened to land in an unsettled pass) are re-diffed
                    // and emitted on the next settled pass. Advancing it
                    // here would drop them permanently.
                    tracing::warn!(
                        "shared doc {stid}: snapshot unsettled — watermark frozen; deletes \
                         withheld until the next settled projection pass"
                    );
                }
                Ok(())
            }
        },
    );
    ProjectionWorker { _handle: handle }
}

/// One share-projection diff step: snapshot `after` settled-aware, apply the
/// share `patch`, diff against `before`, and WITHHOLD delete ops when the
/// snapshot is unsettled — mirroring the main-doc gate in
/// `LoroSyncController` (loro_sync_controller.rs delete-pass gate).
///
/// Why: an unsettled snapshot under-reports the live set (a node was
/// transiently meta-incomplete or missing its fractional index). Diffing it
/// naively makes that withheld node look "gone" and would emit a REAL SQL
/// DELETE for a block that is alive in the shared Loro tree. Legitimate
/// deletes (node parented to Deleted/Unexist) keep the snapshot settled and
/// still flow.
fn share_diff_ops(
    doc: &loro::LoroDoc,
    before: &HashMap<String, crate::loro_backend::SnapshotBlock>,
    patch: &impl Fn(&mut HashMap<String, crate::loro_backend::SnapshotBlock>),
    stid: &str,
) -> (Vec<(String, StorageEntity)>, bool) {
    use crate::loro_backend::snapshot_blocks_from_doc_settled;
    use crate::loro_sync_controller::diff_snapshots_to_ops;
    let (mut after, after_settled) = snapshot_blocks_from_doc_settled(doc);
    patch(&mut after);
    let mut ops = diff_snapshots_to_ops(before, &after);
    if !after_settled {
        let before_len = ops.len();
        ops.retain(|(name, _)| name != "delete");
        let withheld = before_len - ops.len();
        if withheld > 0 {
            tracing::warn!(
                "shared doc {stid}: withholding {withheld} delete(s) — snapshot unsettled (a live \
                 node was transiently unreadable or missing its fractional index); real deletes \
                 flow on the next settled pass"
            );
        }
    }
    (ops, after_settled)
}

impl LoroShareBackend {
    /// Construct a new backend. Always returns `Arc<Self>` — the
    /// internal `self_weak` field requires an `Arc` context, so a bare
    /// `Self` value can't exist. `Arc::new_cyclic` gives us the
    /// `Weak<Self>` BEFORE the `Arc` is fully assembled, letting us
    /// populate the field in the same statement that constructs the
    /// `Arc`. No post-construction registration, no runtime lock.
    pub fn new(
        store: Arc<RwLock<LoroDocumentStore>>,
        snapshot_store: Arc<SharedSnapshotStore>,
        manager: Arc<SharedTreeSyncManager>,
        advertiser: Arc<IrohAdvertiser>,
        degraded_bus: Arc<DegradedSignalBus>,
        device_key: SecretKey,
    ) -> Arc<Self> {
        Self::new_with_sql(
            store,
            snapshot_store,
            manager,
            advertiser,
            degraded_bus,
            device_key,
            None,
            None,
        )
    }

    /// Construct with an explicit SQL operation provider. The DI-wired path
    /// uses this so mount-node projection can write Block rows; tests that
    /// don't need UI visibility use [`new`] with `sql_ops = None`.
    // Grouping these into a params struct would ripple into the caller in
    // `crates/holon/src/sync/loro_module.rs`, outside this crate.
    #[allow(clippy::too_many_arguments)]
    pub fn new_with_sql(
        store: Arc<RwLock<LoroDocumentStore>>,
        snapshot_store: Arc<SharedSnapshotStore>,
        manager: Arc<SharedTreeSyncManager>,
        advertiser: Arc<IrohAdvertiser>,
        degraded_bus: Arc<DegradedSignalBus>,
        device_key: SecretKey,
        sql_ops: Option<Arc<dyn OriginTaggedWrites>>,
        downstream_projection: Option<Arc<dyn DownstreamProjection>>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|self_weak| Self {
            store,
            snapshot_store,
            manager,
            advertiser,
            degraded_bus,
            device_key,
            sql_ops,
            downstream_projection,
            known_peers: Arc::new(RwLock::new(HashMap::new())),
            save_workers: Arc::new(RwLock::new(HashMap::new())),
            sync_workers: Arc::new(RwLock::new(HashMap::new())),
            projection_workers: Arc::new(RwLock::new(HashMap::new())),
            self_weak: self_weak.clone(),
        })
    }

    /// Clone the weak self-reference installed during construction.
    fn weak_self(&self) -> std::sync::Weak<LoroShareBackend> {
        self.self_weak.clone()
    }

    /// Attach (or replace) the save worker for a given shared doc.
    /// Called by `share_subtree`, `accept_shared_subtree`, and the
    /// startup rehydration path.
    pub async fn attach_save_worker(&self, shared_tree_id: String, doc: Arc<LoroDoc>) {
        let worker = spawn_save_worker(
            self.snapshot_store.clone(),
            self.degraded_bus.clone(),
            shared_tree_id.clone(),
            doc,
        );
        self.save_workers
            .write()
            .await
            .insert(shared_tree_id, worker);
    }

    /// Attach (or replace) the auto-resync worker for a given shared
    /// doc. Subscribes to `subscribe_root` and, on non-remote commits,
    /// debounces + calls `sync_with_peers`. Called wherever
    /// `attach_save_worker` is called.
    pub async fn attach_sync_worker(&self, shared_tree_id: String, doc: Arc<LoroDoc>) {
        // Read the doc's real peer id (already set via `set_peer_id`
        // before this worker is attached). Recomputing `stable_peer_id`
        // here would re-bump the generation and diverge from the id the
        // doc actually authors under.
        let local_peer_id = doc.peer_id();
        let worker =
            spawn_sync_worker(self.weak_self(), shared_tree_id.clone(), doc, local_peer_id);
        self.sync_workers
            .write()
            .await
            .insert(shared_tree_id, worker);
    }

    /// Attach (or replace) the SQL projection worker for a given shared
    /// doc. On every change (local or imported), diffs the doc against
    /// a watermark and writes the delta into the SQL block table.
    /// No-op when `sql_ops` is `None` (tests without the DI stack).
    pub async fn attach_projection_worker(
        &self,
        shared_tree_id: String,
        doc: Arc<LoroDoc>,
        mount_block_uri: String,
    ) -> Result<()> {
        let Some(sql_ops) = self.sql_ops.as_ref() else {
            return Ok(());
        };
        let global_doc = self.global_doc().await?;
        let worker = spawn_projection_worker(
            doc,
            sql_ops.clone(),
            self.degraded_bus.clone(),
            mount_block_uri,
            shared_tree_id.clone(),
            global_doc,
        );
        self.projection_workers
            .write()
            .await
            .insert(shared_tree_id, worker);
        Ok(())
    }

    /// Persist every shared doc currently registered. Called on
    /// graceful shutdown so we don't rely on the debounce window
    /// flushing before process exit. Failures are logged + emitted
    /// but do not abort the flush of the remaining shares.
    pub async fn flush_all(&self) {
        let snapshots: Vec<(String, Arc<LoroDoc>)> = {
            let guard = self.save_workers.read().await;
            guard
                .iter()
                .map(|(id, w)| (id.clone(), w.doc.clone()))
                .collect()
        };
        for (id, doc) in snapshots {
            if let Err(e) = self.snapshot_store.save(&id, &doc) {
                warn!(
                    shared_tree_id = %id,
                    error = %e,
                    "[share] flush_all: save failed"
                );
                self.degraded_bus.emit(ShareDegraded {
                    shared_tree_id: id,
                    reason: ShareDegradedReason::SnapshotSaveFailed(format!("{e:#}")),
                });
            }
        }
    }

    /// Snapshot store accessor — used by `rehydrate_shared_trees` so
    /// it doesn't need a separate copy of the `Arc`.
    pub fn snapshot_store(&self) -> &Arc<SharedSnapshotStore> {
        &self.snapshot_store
    }

    /// Degraded-mode bus accessor — rehydration uses this to emit
    /// `RehydrationFailed` for shares that load successfully but fail
    /// to re-advertise.
    pub fn degraded_bus(&self) -> &Arc<DegradedSignalBus> {
        &self.degraded_bus
    }

    /// Project a mount node into the SQL `block` table.
    ///
    /// The UI reads from SQL matviews, not from Loro, so a mount node
    /// that exists only in the Loro tree is invisible. This writes a
    /// Block row with:
    /// - `id = mount_block_uri` (e.g. `block:<uuid>`)
    /// - `parent_id = parent_block_uri`
    /// - `content = fallback_title` (placeholder until descendant projection
    ///   fills in the real content)
    /// - `share-role = "mount"` and `shared-tree-id = <uuid>` packed into the
    ///   `properties` JSON column, so downstream queries can locate mount rows
    ///   without traversing Loro metadata.
    ///
    /// Uses the SQL operation provider's `create` op, which emits an
    /// `EventKind::Created` — `CacheEventSubscriber` picks it up and
    /// refreshes `QueryableCache<Block>` immediately.
    ///
    /// When `sql_ops` is `None` (tests without DI wiring), this is a
    /// no-op — mount nodes only make it into Loro, which is what the
    /// backend-only tests want.
    async fn project_mount_to_sql(
        &self,
        mount_block_uri: &str,
        parent_block_uri: &str,
        shared_tree_id: &str,
        fallback_title: &str,
    ) -> Result<()> {
        let Some(sql_ops) = self.sql_ops.as_ref() else {
            return Ok(());
        };
        let mut params = StorageEntity::new();
        params.insert("id".into(), Value::String(mount_block_uri.to_string()));
        params.insert(
            "parent_id".into(),
            Value::String(parent_block_uri.to_string()),
        );
        params.insert("content".into(), Value::String(fallback_title.to_string()));
        params.insert("content_type".into(), Value::String("text".to_string()));
        // The mount is a Page (Inc 2) — it owns a dedicated on-disk org file so
        // the shared subtree's write-back resolves a path (`name_chain`
        // terminates at the mount) instead of inlining shared content into a
        // global-truth file. `tags: ["Page"]` writes the `block_tags` junction
        // the create op consumes. The caller has already parented the mount
        // under a page ancestor (Amendment A), so this never lands a page under
        // a non-page.
        params.insert(
            "tags".into(),
            Value::Array(vec![Value::String(holon_api::block::PAGE_TAG.to_string())]),
        );
        // Custom properties — `SqlOperationProvider::prepare_create` packs
        // any key not in `BLOCKS_KNOWN_COLUMNS` into the `properties` JSON.
        params.insert(
            SHARE_ROLE_PROPERTY.into(),
            Value::String(SHARE_ROLE_MOUNT.to_string()),
        );
        params.insert(
            SHARED_TREE_ID_PROPERTY.into(),
            Value::String(shared_tree_id.to_string()),
        );

        let entity = EntityName::new("block");
        sql_ops
            .execute_operation(&entity, "create", params)
            .await
            .map_err(|e| err(format!("project mount node into SQL: {e}")))?;
        Ok(())
    }

    /// Project the recipient-side **"Shared with me" root** into the SQL
    /// `block` table so the UI (which reads SQL matviews, not Loro) renders
    /// it as a top-level page. Idempotent: uses the `create` op's `INSERT
    /// OR IGNORE` semantics, so re-projecting on every accept leaves an
    /// existing row untouched. No-op without DI-wired `sql_ops`
    /// (backend-only tests).
    ///
    /// Pairs with [`ensure_shared_with_me_root_node`] (the Loro side): together
    /// they give an accepted mount a visible, reachable home (ADR 0028 H7).
    async fn project_shared_with_me_root_to_sql(&self) -> Result<()> {
        let Some(sql_ops) = self.sql_ops.as_ref() else {
            return Ok(());
        };
        let root_uri = block_uri_from_bare(SHARED_WITH_ME_ROOT_ID);
        let mut params = StorageEntity::new();
        params.insert("id".into(), Value::String(root_uri));
        params.insert(
            "parent_id".into(),
            Value::String(EntityUri::no_parent().as_str().to_string()),
        );
        params.insert(
            "content".into(),
            Value::String(SHARED_WITH_ME_TITLE.to_string()),
        );
        params.insert("content_type".into(), Value::String("text".to_string()));
        params.insert(
            "tags".into(),
            Value::Array(vec![Value::String(holon_api::block::PAGE_TAG.to_string())]),
        );
        let entity = EntityName::new("block");
        sql_ops
            .execute_operation(&entity, "create", params)
            .await
            .map_err(|e| err(format!("project 'Shared with me' root into SQL: {e}")))?;
        Ok(())
    }

    /// Project all nodes from a shared LoroDoc into SQL block rows.
    ///
    /// Reuses the standard `snapshot_blocks_from_doc` → `block_to_params`
    /// pipeline. Each block is patched to: (a) remap the shared root's
    /// parent from `no_parent` to the mount block URI, and (b) stamp
    /// `shared-tree-id` into properties so downstream queries/routing
    /// can identify blocks belonging to this share.
    ///
    /// Uses `INSERT OR IGNORE` semantics (via the `create` op) so this
    /// is idempotent across accept + rehydrate.
    async fn project_descendants_to_sql(
        &self,
        shared_doc: &LoroDoc,
        mount_block_uri: &str,
        shared_tree_id: &str,
    ) -> Result<()> {
        let Some(sql_ops) = self.sql_ops.as_ref() else {
            return Ok(());
        };
        let mount_uri = EntityUri::parse(mount_block_uri)
            .map_err(|e| err(format!("bad mount URI {mount_block_uri:?}: {e:#}")))?;
        let stid = shared_tree_id.to_string();

        // D3 mount-identity shaping. The shared subtree's root determines how it
        // maps onto the mount page:
        //   * page share (root is a page P): ADOPT-AND-COLLAPSE — the mount page IS P
        //     (project_mount_to_sql adopted P's title + Page tag), so P's node folds:
        //     reparent P's CHILDREN to the mount and DROP P's own row. P stays
        //     uncollapsed in the shared Loro doc (CRDT truth) — the fold is
        //     projection-only (deliberate Loro↔SQL shape difference, mapped in the
        //     keystone oracles).
        //   * block share (root is a plain block): SYNTHETIC CONTAINER — the mount is a
        //     synthetic page wrapping the shared block; reparent the root (its
        //     `no_parent`) to the mount and project it unchanged.
        let root_blocks = crate::loro_backend::snapshot_blocks_from_doc(shared_doc);
        let root = root_blocks
            .values()
            .find(|s| s.block.parent_id.is_no_parent() || s.block.parent_id.is_sentinel())
            .ok_or_else(|| err("shared doc has no root node for projection".to_string()))?;
        let root_is_page = root.block.is_page();
        let root_id = root.block.id.clone();

        let mut ops = project_shared_doc_to_ops(shared_doc, |block| {
            block
                .properties
                .entry(SHARED_TREE_ID_PROPERTY.to_string())
                .or_insert_with(|| Value::String(stid.clone()));
            if root_is_page {
                // Fold P onto the mount: P's direct children reparent to it.
                if block.parent_id == root_id {
                    block.parent_id = mount_uri.clone();
                }
            } else if block.parent_id.is_no_parent() || block.parent_id.is_sentinel() {
                block.parent_id = mount_uri.clone();
            }
        });
        if root_is_page {
            // Drop P's own row — its identity now lives on the mount page.
            let root_id_str = root_id.as_str();
            ops.retain(|(_, params)| {
                params.get("id").and_then(|v| v.as_string()) != Some(root_id_str)
            });
        }
        if ops.is_empty() {
            return Ok(());
        }
        // Integrity guard (see `first_local_collision`): a shared doc must never
        // project a block whose id shadows a LIVE local block. This runs at
        // accept/re-project time; a collision means a hostile or corrupt shared
        // doc, so reject the whole projection loudly rather than clobber local
        // SQL rows.
        let global = self.global_doc().await?;
        if let Some(bad) = global
            .with_read(|d| Ok(first_local_collision(d, &ops)))
            .map_err(|e| err(format!("global-doc read for collision guard: {e:#}")))?
        {
            return Err(err(format!(
                "shared doc {shared_tree_id} projection refused: block id {bad:?} collides with a \
                 live local block — refusing to shadow local content"
            )));
        }
        let entity = EntityName::new("block");
        for (op_name, params) in ops {
            sql_ops
                .execute_operation(&entity, &op_name, params)
                .await
                .map_err(|e| err(format!("project descendant into SQL ({op_name}): {e}")))?;
        }
        Ok(())
    }

    /// Whether `block_id` is an AUTHORITATIVELY-registered shared-subtree mount
    /// — i.e. a real mount NODE exists for it in the global Loro tree
    /// (created by `share_subtree`/`accept_shared_subtree`, carrying
    /// non-user-authorable mount metadata). This is the sound signal the
    /// org write-back ingest guard consults so a hand-authored file that
    /// merely carries a `:share-role: mount:` drawer property (which
    /// round-trips into SQL) is still ingested normally instead of being
    /// silently skipped (data loss). The global doc is loaded before the
    /// org ingest sweep, so this is answerable at first ingest.
    pub async fn is_registered_mount(&self, block_id: &str) -> Result<bool> {
        let bare = block_id.strip_prefix("block:").unwrap_or(block_id);
        let collab = self.global_doc().await?;
        let doc_arc = collab.doc();
        let doc = &*doc_arc;
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        for node in tree.get_nodes(false) {
            if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
                continue;
            }
            if shared_tree::is_mount_node(&tree, node.id)
                && let Some(sid) = read_stable_id(&tree, node.id)
            {
                let sid_bare = sid.strip_prefix("block:").unwrap_or(&sid);
                if sid_bare == bare {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }

    async fn global_doc(&self) -> Result<Arc<crate::loro_document::LoroDocument>> {
        let store = self.store.read().await;
        store
            .get_global_doc()
            .await
            .map_err(|e| err(format!("get_global_doc failed: {e:#}")))
    }

    async fn remember_peer(&self, shared_tree_id: &str, addr: EndpointAddr) {
        let persisted = {
            let mut guard = self.known_peers.write().await;
            let entry = guard.entry(shared_tree_id.to_string()).or_default();
            // Merge into any existing entry for the same `EndpointId`.
            // The id is stable across restarts (derived from the
            // device key) and so is the advertiser port (persisted
            // sidecar, see `start_advertising_stable`), so older
            // socket addrs usually stay valid — and the freshly
            // observed addr can be a single non-routable interface
            // (e.g. a vmnet host-only addr), so REPLACING the set
            // with it would throw away the dialable routes. iroh
            // races all candidate paths, so unioning is safe.
            if let Some(pos) = entry.iter().position(|a| a.id == addr.id) {
                entry[pos].addrs.extend(addr.addrs);
            } else {
                entry.push(addr);
            }
            tracing::debug!(
                shared_tree_id = %shared_tree_id,
                peers = ?entry,
                "[share] remember_peer updated known_peers"
            );
            entry.clone()
        };
        if let Err(e) = self.snapshot_store.save_peers(shared_tree_id, &persisted) {
            // Not fatal — the in-memory entry is authoritative while
            // the process runs. Surface as degraded so the user knows
            // cross-peer sync after restart may regress.
            warn!(
                shared_tree_id = %shared_tree_id,
                error = %e,
                "[share] save_peers failed"
            );
            self.degraded_bus.emit(ShareDegraded {
                shared_tree_id: shared_tree_id.to_string(),
                reason: ShareDegradedReason::SnapshotSaveFailed(format!(
                    "peers sidecar save failed: {e:#}"
                )),
            });
        }
    }

    /// Build a peer-connected callback that remembers every inbound
    /// dialer's address on this backend. Returns an `OnPeerConnected`
    /// suitable for `IrohAdvertiser::start_share_with_callback`.
    fn peer_connected_callback(&self) -> OnPeerConnected {
        let weak = self.weak_self();
        Arc::new(move |shared_tree_id: String, addr: EndpointAddr| {
            let Some(strong) = weak.upgrade() else {
                return;
            };
            tokio::spawn(async move {
                strong.remember_peer(&shared_tree_id, addr).await;
            });
        })
    }

    /// Start advertising with a restart-stable UDP port.
    ///
    /// Loads the port sidecar (if any), binds the advertiser to it, and
    /// persists the actually-bound port for the next restart. Keeping
    /// the port stable is what keeps the addrs peers persisted for us
    /// dialable across our restarts — relay and discovery are disabled,
    /// so a changed port partitions us until WE dial THEM first.
    async fn start_advertising_stable(
        &self,
        shared_tree_id: &str,
        doc: Arc<LoroDoc>,
    ) -> anyhow::Result<EndpointAddr> {
        let preferred_port = match self.snapshot_store.load_port(shared_tree_id) {
            Ok(p) => p,
            Err(e) => {
                warn!(
                    shared_tree_id = %shared_tree_id,
                    error = %e,
                    "[share] port sidecar unreadable; binding ephemeral port"
                );
                None
            }
        };
        let addr = self
            .advertiser
            .start_share_with_callback(
                shared_tree_id.to_string(),
                doc,
                Some(self.peer_connected_callback()),
                preferred_port,
                // Enrollment gate not yet flipped ON in the backend hot path
                // (share_subtree/accept/resync/rehydrate must all enroll in
                // lockstep first — the enrollment MECHANISM lands here and is
                // proven at the transport layer; wiring the backend to pass a
                // real roster is the remaining integration). Un-gated = legacy.
                None,
            )
            .await?;
        let bound_port = addr.addrs.iter().find_map(|t| match t {
            iroh::TransportAddr::Ip(sa) if sa.is_ipv4() => Some(sa.port()),
            _ => None,
        });
        match bound_port {
            Some(port) => {
                if let Err(e) = self.snapshot_store.save_port(shared_tree_id, port) {
                    warn!(
                        shared_tree_id = %shared_tree_id,
                        port = port,
                        error = %e,
                        "[share] save_port failed — next restart rebinds ephemeral"
                    );
                }
            }
            None => warn!(
                shared_tree_id = %shared_tree_id,
                addr = ?addr,
                "[share] no IPv4 addr on advertiser endpoint — port not persisted"
            ),
        }
        Ok(addr)
    }

    /// Sync bidirectionally with every known peer for `shared_tree_id`.
    /// The initiator side of the VV-based protocol pushes our updates
    /// and pulls theirs in one round — so a single call on either side
    /// converges both peers (cf. `sync_doc_initiate`).
    ///
    /// Prefers the advertiser's long-lived endpoint for dialing so
    /// that the remote side's accept-loop callback records a *dialable*
    /// addr for us (not a short-lived client-only endpoint that dies
    /// after the sync). Falls back to a fresh endpoint when the
    /// advertiser has no endpoint for this share.
    ///
    /// Renamed from `pull_from_peers` to reflect bidirectional semantics.
    pub async fn sync_with_peers(&self, shared_tree_id: &str) -> Result<usize> {
        let doc = self
            .manager
            .get_doc(shared_tree_id)
            .ok_or_else(|| err(format!("no shared doc registered for {shared_tree_id}")))?;
        let peers = {
            let guard = self.known_peers.read().await;
            guard.get(shared_tree_id).cloned().unwrap_or_default()
        };
        // Save-before-push barrier: persist the shared doc so any ops we
        // are about to push are already durable on disk. A crash right
        // after a push then always finds the pushed ops locally, so we
        // never advertise ops we could lose. Complements (does not
        // replace) the debounced save worker.
        if let Err(e) = self.snapshot_store.save(shared_tree_id, &doc) {
            self.degraded_bus.emit(ShareDegraded {
                shared_tree_id: shared_tree_id.to_string(),
                reason: ShareDegradedReason::SnapshotSaveFailed(format!(
                    "pre-push save failed: {e:#}"
                )),
            });
            return Err(err(format!(
                "pre-push snapshot save failed; refusing to push un-persisted ops: {e:#}"
            )));
        }
        let alpn_bytes = make_alpn(ALPN_PREFIX, shared_tree_id);
        let advertiser_ep = self.advertiser.endpoint_for(shared_tree_id).await;
        let mut synced = 0usize;
        for addr in peers {
            let ep = match advertiser_ep.clone() {
                Some(ep) => ep,
                None => create_endpoint(vec![alpn_bytes.clone()])
                    .await
                    .map_err(|e| err(format!("create endpoint: {e:#}")))?,
            };
            tracing::debug!(
                shared_tree_id = %shared_tree_id,
                local_endpoint = %ep.id().fmt_short(),
                dial_addr = ?addr,
                "[share] dialing peer"
            );
            let fut = sync_doc_initiate(&ep, &doc, &alpn_bytes, addr);
            match timeout(CONNECT_TIMEOUT, fut).await {
                Ok(Ok(conn)) => {
                    // `sync_doc_initiate` now drains the recv stream
                    // until the acceptor's EOF before returning, so by
                    // this point the QUIC streams are provably closed
                    // from both ends — the acceptor has imported our
                    // delta and acknowledged it. Dropping the
                    // `Connection` here is safe.
                    drop(conn);
                    synced += 1;
                }
                Ok(Err(e)) => warn!("[share] sync with peer failed: {e:#}"),
                Err(_) => warn!("[share] sync with peer timed out"),
            }
        }
        Ok(synced)
    }

    /// Test-only access to the global Loro document. Kept behind a
    /// separate name so production code doesn't accidentally reach past
    /// the operation surface.
    pub async fn test_global_doc(&self) -> Arc<crate::loro_document::LoroDocument> {
        self.global_doc().await.expect("test global_doc")
    }

    /// Test-only access to the shared-tree manager (to fetch shared docs).
    pub fn manager_for_test(&self) -> Arc<SharedTreeSyncManager> {
        self.manager.clone()
    }

    /// Test-only access to the advertiser (for teardown in tests).
    pub fn advertiser_for_test(&self) -> Arc<IrohAdvertiser> {
        self.advertiser.clone()
    }
}

/// Interim N4 guard (dogfood 2026-07-20). Sharing the **root layout block** or
/// the **default-document root** wraps the ENTIRE UI (sidebars, panels, advice,
/// render/src blocks) under a share mount — the frontend collapses to a blank
/// screen and, absent the write-back removal guard, the on-disk vault can be
/// destroyed. These are structural/layout blocks, never user
/// content, so sharing them is always a mistake. Reject loudly.
///
/// This is deliberately cheap and id-based. ADR 0028 replaces the mechanism
/// with C3 fail-closed boundary classification (every structural op declares
/// its boundary behavior); until then this closes the vault-destroying hole.
fn structural_share_rejection(id: &EntityUri) -> Option<String> {
    let s = id.as_str();
    if s == holon_api::ROOT_LAYOUT_BLOCK_ID {
        return Some(format!(
            "refusing to share {s}: it is the root layout block. Sharing it would wrap the entire \
             UI (sidebars, panels, advice) under a share mount and collapse the app to a blank \
             screen. Share a specific page or block instead."
        ));
    }
    if s == holon_api::DEFAULT_DOC_BLOCK_ID {
        return Some(format!(
            "refusing to share {s}: it is the default-document root that owns the bundled layout. \
             Sharing it would pull the whole UI layout into the share. Share a specific page or \
             block instead."
        ));
    }
    None
}

fn parse_retention(s: &str) -> Result<HistoryRetention> {
    match s {
        "none" => Ok(HistoryRetention::None),
        // `HistoryRetention::Full` exports the full forked oplog, which retains
        // the content/history of the pruned NON-subtree nodes (the rest of the
        // vault). Sharing that is a whole-vault history leak, so it is disabled
        // at the boundary. The variant is kept for internal/test use only.
        // See docs/Reference/SUBTREE_SHARING.md B1.
        "full" => Err(err("retention 'full' is disabled: it would leak the \
                           content and history of your other (non-shared) notes \
                           to the recipient. Use 'none' (state-only sharing).")),
        "since" => Err(err("retention 'since' is not yet supported; use 'none'")),
        other => Err(err(format!(
            "unknown retention '{other}' (expected 'none')"
        ))),
    }
}

/// Flat scan of the tree for a node with matching STABLE_ID metadata.
/// Kept local to avoid pulling the full LoroBackend cache machinery.
/// Find a tree node by its `STABLE_ID` metadata.
///
/// Callers must pass an already-parsed [`EntityUri`] so the string/scheme
/// validation happens at the external boundary (parse, don't validate). Loro
/// stores `STABLE_ID` as the bare path component of the URI (the UUID for
/// block URIs), and this function compares on that canonical form — there is
/// no ambiguity over "full URI vs bare id" at the call site.
fn find_tree_id_by_stable_id(doc: &LoroDoc, stable_id: &EntityUri) -> Option<TreeID> {
    let needle = stable_id.id();
    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
    for node in tree.get_nodes(false) {
        if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
            continue;
        }
        if let Ok(meta) = tree.get_meta(node.id)
            && let Some(ValueOrContainer::Value(v)) = meta.get(STABLE_ID)
            && v.as_string().map(|s| s.as_str()) == Some(needle)
        {
            return Some(node.id);
        }
    }
    None
}

/// Return the first projected op id that collides with a LIVE node in the
/// recipient's global tree, if any.
///
/// A shared subtree's descendants are pruned from the global tree when the
/// subtree is shared, so under honest operation NO projected id is alive in the
/// global tree. A collision therefore means the shared doc is trying to
/// *shadow* a LOCAL block id — e.g. a hostile sharer naming a node
/// `block:journals`. Because SQL `block` rows are keyed by these ids,
/// projecting such an op would let the remote peer's `update`/`delete` clobber
/// the recipient's own row (the UI reads SQL). The global tree is the authority
/// for local block identity (SQL is projected from it), so it is the correct
/// place to detect the shadow. Callers fail loud instead of clobbering.
fn first_local_collision(global: &LoroDoc, ops: &[(String, StorageEntity)]) -> Option<String> {
    for (_op, params) in ops {
        if let Some(Value::String(id)) = params.get("id") {
            // op id string is produced by block_to_params for the SQL op batch
            // ALLOW(entity_uri_from_raw): canonical block URI from block_to_params
            let uri = EntityUri::from_raw(id);
            if find_tree_id_by_stable_id(global, &uri).is_some() {
                return Some(id.clone());
            }
        }
    }
    None
}

/// Find an existing mount node for a given `shared_tree_id`. Returns the
/// mount's `TreeID` and its `STABLE_ID` if found.
fn find_mount_by_shared_tree_id(doc: &LoroDoc, shared_tree_id: &str) -> Option<(TreeID, String)> {
    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
    for node in tree.get_nodes(false) {
        if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
            continue;
        }
        if !shared_tree::is_mount_node(&tree, node.id) {
            continue;
        }
        if let Some(info) = shared_tree::read_mount_info(&tree, node.id)
            && info.shared_tree_id == shared_tree_id
        {
            let stable_id = read_stable_id(&tree, node.id)
                .map(|s| block_uri_from_bare(&s))
                .unwrap_or_default();
            return Some((node.id, stable_id));
        }
    }
    None
}

fn parent_as_option(doc: &LoroDoc, tid: TreeID) -> Option<TreeID> {
    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
    match tree.parent(tid) {
        Some(TreeParentId::Node(p)) => Some(p),
        _ => None,
    }
}

/// The nearest `Page`-tagged ancestor of `tid` in the global tree, INCLUSIVE
/// of `tid` itself. `Ok(Some(page_tid))` when a page is found; `Ok(None)` when
/// the walk reaches a root without hitting a page (the mount becomes
/// top-level).
///
/// This is the Loro-side of Amendment A: a mount is tagged a Page, and a page
/// may not sit under a non-page (interim ruling 2026-07-13). Parenting the
/// mount here keeps the Loro tree and the SQL projection aligned — in the
/// common case (sharing a page / a block already under a page) `tid` IS a page
/// and the mount stays in place; only a block shared under a non-page bubbles.
fn nearest_page_ancestor_tid(doc: &LoroDoc, tid: TreeID) -> Result<Option<TreeID>> {
    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
    let mut cur = tid;
    loop {
        if crate::loro_backend::node_is_page(&tree, cur).map_err(|e| err(format!("{e:#}")))? {
            return Ok(Some(cur));
        }
        match parent_as_option(doc, cur) {
            Some(p) => cur = p,
            None => return Ok(None),
        }
    }
}

/// Ensure the recipient-side **"Shared with me" root** exists in the global
/// tree and return its `TreeID` (ADR 0028 H7). The root is a top-level `Page`
/// node keyed by the well-known [`SHARED_WITH_ME_ROOT_ID`], so this is
/// idempotent: an existing root is returned unchanged; only the first accept on
/// a device mints it. Accepted-share mounts that have no page ancestor to sit
/// under attach here instead of orphaning at `no_parent` (invisible in the UI).
///
/// The caller must hold the global-doc write lock; this both reads and, on
/// first use, writes+commits the new node.
fn ensure_shared_with_me_root_node(doc: &LoroDoc) -> Result<TreeID> {
    let uri = EntityUri::block(SHARED_WITH_ME_ROOT_ID);
    if let Some(tid) = find_tree_id_by_stable_id(doc, &uri) {
        return Ok(tid);
    }
    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
    let node = tree
        .create(None)
        .map_err(|e| err(format!("create 'Shared with me' root node: {e:#}")))?;
    let meta = tree
        .get_meta(node)
        .map_err(|e| err(format!("get 'Shared with me' root meta: {e:#}")))?;
    meta.insert(STABLE_ID, SHARED_WITH_ME_ROOT_ID)
        .map_err(|e| err(format!("set 'Shared with me' stable_id: {e:#}")))?;
    let text = crate::mergeable_child::ensure_text(&meta, "content_raw")
        .map_err(|e| err(format!("insert 'Shared with me' content: {e:#}")))?;
    text.insert(0, SHARED_WITH_ME_TITLE)
        .map_err(|e| err(format!("write 'Shared with me' title: {e:#}")))?;
    let tags_json = serde_json::to_string(&[holon_api::block::PAGE_TAG])
        .map_err(|e| err(format!("encode 'Shared with me' tags: {e:#}")))?;
    meta.insert("tags", tags_json.as_str())
        .map_err(|e| err(format!("tag 'Shared with me' as Page: {e:#}")))?;
    doc.commit();
    Ok(node)
}

/// Summary of a shared doc's root node used to shape materialization (D3):
/// `is_page` decides adopt-and-collapse (page share — the mount adopts P's
/// identity, P's node folds) vs synthetic-container (block share — the mount is
/// a synthetic page wrapping the shared block). `title` is the root's content,
/// adopted as the mount page's title in the page case.
///
/// The shared subtree has exactly one root (extract_for_share reparents the
/// subtree root to the tree root); zero or many roots is a corrupt share.
fn shared_root_summary(shared_doc: &LoroDoc) -> Result<(bool, String)> {
    let blocks = crate::loro_backend::snapshot_blocks_from_doc(shared_doc);
    let mut roots = blocks
        .values()
        .filter(|s| s.block.parent_id.is_no_parent() || s.block.parent_id.is_sentinel());
    let root = roots
        .next()
        .ok_or_else(|| err("shared doc has no root node".to_string()))?;
    if roots.next().is_some() {
        return Err(err(
            "shared doc has multiple root nodes; expected exactly one shared subtree root"
                .to_string(),
        ));
    }
    Ok((root.block.is_page(), root.block.content.clone()))
}

fn set_stable_id(doc: &LoroDoc, tid: TreeID, stable_id: &str) -> anyhow::Result<()> {
    // `STABLE_ID` metadata is the **bare** id (no `block:` prefix) — the
    // rest of the stack (`find_tree_id_by_stable_id`, `resolve_to_tree_id`,
    // `set_external_id`) all assume this and strip prefixes on the read
    // side. Strip here too so a single-pass write matches every lookup.
    let bare = stable_id.strip_prefix("block:").unwrap_or(stable_id);
    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
    let meta = tree.get_meta(tid)?;
    meta.insert(STABLE_ID, bare)?;
    Ok(())
}

/// True if `e` is the `IrohAdvertiser` "already advertising" error —
/// which is semantically success on the rehydration / accept paths.
fn is_already_advertising(e: &anyhow::Error) -> bool {
    format!("{e:#}").contains("is already being advertised")
}

#[async_trait]
impl SubtreeShareOperations<()> for LoroShareBackend {
    async fn share_subtree(&self, id: &str, retention: String) -> Result<OperationResult> {
        let id_uri =
            EntityUri::parse(id).map_err(|e| err(format!("invalid block URI {id:?}: {e:#}")))?;
        if !id_uri.is_block() {
            return Err(err(format!(
                "share_subtree expects a `block:` URI, got scheme {:?} (full URI: {id:?})",
                id_uri.scheme()
            )));
        }
        // N4 interim guard: never share the layout/structural roots.
        if let Some(reason) = structural_share_rejection(&id_uri) {
            return Err(err(reason));
        }
        let retention = parse_retention(&retention)?;
        let collab = self.global_doc().await?;
        let shared_tree_id = Uuid::new_v4().to_string();
        let mount_stable_id = format!("block:{}", Uuid::new_v4());

        // Phase A (non-destructive) + snapshot save + Phase B (destructive)
        // all happen under the same global-doc write lock. Phase A
        // forks the shared doc; the snapshot is written to disk before
        // we mutate the source tree. If the save fails, Phase B never
        // runs and the source stays untouched — no rollback.
        let (shared_arc, shared_root, mount_parent_uri) = {
            let doc_arc = collab.doc();
            let doc = &*doc_arc;

            let tid = find_tree_id_by_stable_id(doc, &id_uri)
                .ok_or_else(|| err(format!("block {id} not found in Loro tree")))?;
            if shared_tree::is_mount_node(&doc.get_tree(crate::loro_backend::TREE_NAME), tid) {
                return Err(err(format!(
                    "block {id} is already a mount node; sharing a mount is not supported"
                )));
            }
            // Amendment A: the mount is a Page (Inc 2), so it must sit under a
            // Page (or a root). Bubble the subtree's original parent to its
            // nearest page ancestor — a no-op in the common case (sharing a
            // page, or a block already under a page), so the mount stays in
            // place; only a block shared under a non-page bubbles up. Using this
            // `parent` for BOTH the SQL mount row and the Loro mount placement
            // keeps the two stores aligned.
            let parent = match parent_as_option(doc, tid) {
                Some(ptid) => nearest_page_ancestor_tid(doc, ptid)?,
                None => None,
            };

            // The mount node replaces the shared subtree, so its SQL parent is
            // the resolved page ancestor (from the parent node's STABLE_ID). A
            // shared root with no page ancestor → the mount becomes a top-level
            // block (`no_parent` sentinel).
            let mount_parent_uri = match parent {
                Some(parent_tid) => {
                    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                    read_stable_id(&tree, parent_tid)
                        .map(|s| block_uri_from_bare(&s))
                        .ok_or_else(|| {
                            err(format!(
                                "shared subtree's parent node {parent_tid:?} has no STABLE_ID; \
                                 cannot resolve the mount's SQL parent"
                            ))
                        })?
                }
                None => EntityUri::no_parent().as_str().to_string(),
            };

            // --- Phase A: fork + extract (source unchanged) ---
            let extracted =
                shared_tree::extract_for_share(doc, tid, parent, shared_tree_id.clone(), retention)
                    .map_err(|e| err(format!("extract_for_share failed: {e:#}")))?;

            // Stable peer id BEFORE save so the persisted snapshot
            // already carries the right identity. Bump the generation so
            // this mint can never reuse a `(peer_id, counter)` from a
            // stale snapshot after a crash — see `share_peer_id`.
            let generation = self
                .snapshot_store
                .next_generation(&shared_tree_id)
                .map_err(|e| err(format!("bump peer-id generation: {e:#}")))?;
            let peer_id = stable_peer_id(&self.device_key, &shared_tree_id, generation);
            extracted
                .shared_doc
                .set_peer_id(peer_id)
                .map_err(|e| err(format!("set_peer_id on shared doc: {e:#}")))?;

            // --- Persist shared snapshot BEFORE prune ---
            if let Err(e) = self
                .snapshot_store
                .save(&shared_tree_id, &extracted.shared_doc)
            {
                self.degraded_bus.emit(ShareDegraded {
                    shared_tree_id: shared_tree_id.clone(),
                    reason: ShareDegradedReason::SnapshotSaveFailed(format!("{e:#}")),
                });
                // Source doc is still untouched — drop the extracted
                // doc and bail out. No rollback needed.
                return Err(err(format!(
                    "initial snapshot save failed; source tree unchanged: {e:#}"
                )));
            }

            // --- Phase B: prune source + create mount node ---
            let shared_root = extracted.shared_root;
            let mount_tid = shared_tree::commit_share_prune(doc, &extracted)
                .map_err(|e| err(format!("commit_share_prune failed: {e:#}")))?;
            set_stable_id(doc, mount_tid, &mount_stable_id)
                .map_err(|e| err(format!("set mount stable_id: {e:#}")))?;
            doc.commit();

            (
                Arc::new(extracted.shared_doc),
                shared_root,
                mount_parent_uri,
            )
        };

        // Flush the global doc so the mount node survives in lockstep
        // with the shared snapshot. Failure here leaves consistent
        // memory but inconsistent disk; emit a degraded signal so the
        // controller's next save cycle reconciles things. Return Err
        // because the caller didn't get a ticket — the op failed.
        if let Err(e) = self.store.read().await.save_all().await {
            self.degraded_bus.emit(ShareDegraded {
                shared_tree_id: shared_tree_id.clone(),
                reason: ShareDegradedReason::SnapshotSaveFailed(format!(
                    "global doc save_all failed after fork-prune: {e:#}"
                )),
            });
            return Err(err(format!("global doc save_all failed: {e:#}")));
        }

        // Sharer-side SQL projection. `accept_shared_subtree` projects the
        // mount + descendants so the UI (which reads SQL, not Loro) renders
        // shared content; `share_subtree` must too, or the sharing peer loses
        // the subtree from the UI until the next restart re-hydrates it.
        //
        // Ordering matters. The prune commit above removed every descendant
        // block from the GLOBAL tree, so the global Loro→SQL projection's next
        // diff emits a DELETE for each. That is a ONE-TIME event: the global
        // projection diffs the global doc against an advancing base, so once
        // the pruned snapshot becomes the base it never re-deletes those ids.
        // If we re-created the descendant rows BEFORE that delete pass ran, the
        // delete would wipe them again — the race that made this omission
        // "safe" to skip. We defeat it deterministically:
        //   1. project the mount row (INSERT OR IGNORE; carries `share-role`);
        //   2. flush the global projection — this acquires its project lock
        //      (serializing with the background loop), drains the pending prune-delete,
        //      and advances its base, so no later pass can re-delete the descendants
        //      (the mount CREATE is IGNOREd, leaving our `share-role` row intact);
        //   3. re-project the descendants under the mount as the LAST write.
        // Without the DI-wired projection (tests) there is no global loop to
        // race, so the flush is simply skipped.
        //
        // D3: when the shared root is a PAGE the mount adopts its title (the
        // mount page IS that page); otherwise it is a synthetic container page.
        let (root_is_page, root_title) = shared_root_summary(&shared_arc)?;
        let mount_title = if root_is_page {
            root_title
        } else {
            format!("Shared tree ({shared_tree_id})")
        };
        self.project_mount_to_sql(
            &mount_stable_id,
            &mount_parent_uri,
            &shared_tree_id,
            &mount_title,
        )
        .await?;
        if let Some(projection) = self.downstream_projection.as_ref() {
            projection.flush().await.map_err(|e| {
                err(format!(
                    "flush global projection before re-projecting shared descendants: {e:#}"
                ))
            })?;
        }
        self.project_descendants_to_sql(&shared_arc, &mount_stable_id, &shared_tree_id)
            .await?;

        self.manager
            .register_arc(shared_tree_id.clone(), shared_arc.clone());

        let addr = self
            .start_advertising_stable(&shared_tree_id, shared_arc.clone())
            .await
            .map_err(|e| err(format!("start advertiser: {e:#}")))?;

        self.attach_save_worker(shared_tree_id.clone(), shared_arc.clone())
            .await;
        self.attach_sync_worker(shared_tree_id.clone(), shared_arc.clone())
            .await;
        self.attach_projection_worker(shared_tree_id.clone(), shared_arc, mount_stable_id.clone())
            .await?;

        let alpn = format!("{ALPN_PREFIX}/{shared_tree_id}");
        // Mint the share's access capability + enrollment deadline. The
        // capability — not the leaky `shared_tree_id` — is the real secret a
        // recipient proves possession of at enrollment (see
        // `share_enrollment`). NOTE: the acceptor-side gate that verifies it is
        // wired with the enrollment-ceremony ruling (ADR 0028 §H5/C1'); until
        // then this is forward-compat plumbing, not an enforced boundary.
        let capability = crate::share_enrollment::CapabilitySecret::generate();
        let expires_at = crate::share_enrollment::ExpiryTime(
            chrono::Utc::now().timestamp() + DEFAULT_ENROLLMENT_WINDOW_SECS,
        );
        let ticket = Ticket::new(shared_tree_id.clone(), addr, alpn, capability, expires_at)
            .encode()
            .map_err(|e| err(format!("encode ticket: {e:#}")))?;

        let response = serde_json::json!({
            "ticket": ticket,
            "shared_tree_id": shared_tree_id,
            "mount_block_id": mount_stable_id,
            "shared_root": format!("{}:{}", shared_root.peer, shared_root.counter),
        });
        Ok(
            OperationResult::irreversible(vec![])
                .with_response(Value::String(response.to_string())),
        )
    }

    async fn accept_shared_subtree(
        &self,
        parent_id: &str,
        ticket: String,
    ) -> Result<OperationResult> {
        let parent_uri = EntityUri::parse(parent_id)
            .map_err(|e| err(format!("invalid parent URI {parent_id:?}: {e:#}")))?;
        if !parent_uri.is_block() {
            return Err(err(format!(
                "accept_shared_subtree expects a `block:` URI as parent, got scheme {:?} (full \
                 URI: {parent_id:?})",
                parent_uri.scheme()
            )));
        }
        // SECURITY (disclosed gap): the v2 ticket's capability is decoded but
        // NOT enforced here yet — acceptance is still ALPN-only. Wiring the
        // acceptor gate is deferred to the enrollment-ceremony ruling (see
        // share_enrollment.rs module docs); until then possession of this
        // ticket string remains a bearer read+write capability.
        let t = Ticket::decode(&ticket).map_err(|e| err(format!("decode ticket: {e:#}")))?;

        // Create a fresh LoroDoc for the shared tree. `configure_text_styles`
        // installs the per-key `ExpandType` policy and must run before any
        // mark is applied — Loro silently latches the first config and
        // returns no-ops on conflicting re-configs (Phase 0.1 spike S3).
        // Without this call, `LoroText::mark` either fails silently or stores
        // the mark in a way `to_delta()` doesn't surface, so reads return
        // empty mark sets even though the writer thinks the mark applied.
        let shared_doc = LoroDoc::new();
        crate::loro_backend::configure_text_styles(&shared_doc);
        let generation = self
            .snapshot_store
            .next_generation(&t.shared_tree_id)
            .map_err(|e| err(format!("bump peer-id generation: {e:#}")))?;
        let peer_id = stable_peer_id(&self.device_key, &t.shared_tree_id, generation);
        shared_doc
            .set_peer_id(peer_id)
            .map_err(|e| err(format!("set_peer_id on shared doc: {e:#}")))?;

        // Remember the ticket author's address so later edits can pull.
        self.remember_peer(&t.shared_tree_id, t.addr.clone()).await;

        // Start our advertiser FIRST so the initial pull dials out
        // from our long-lived endpoint. Otherwise the remote peer's
        // accept-loop callback records the short-lived dialer endpoint
        // addr — which dies the moment the sync completes, leaving A
        // with a useless stale addr for B.
        let shared_arc = Arc::new(shared_doc);
        let shared_tree_id = t.shared_tree_id.clone();
        match self
            .start_advertising_stable(&shared_tree_id, shared_arc.clone())
            .await
        {
            Ok(_) => {}
            Err(e) if is_already_advertising(&e) => {
                warn!(
                    shared_tree_id = %shared_tree_id,
                    "[share] advertiser already active; reusing existing share"
                );
            }
            Err(e) => {
                warn!("[share] advertiser start_share failed: {e:#}");
            }
        }
        let alpn_bytes = make_alpn(ALPN_PREFIX, &shared_tree_id);
        let client_ep = self
            .advertiser
            .endpoint_for(&shared_tree_id)
            .await
            .ok_or_else(|| err("advertiser endpoint missing right after start_share"))?;
        let initiate = sync_doc_initiate(&client_ep, &shared_arc, &alpn_bytes, t.addr.clone());
        let _conn = timeout(CONNECT_TIMEOUT, initiate)
            .await
            .map_err(|_| err("initial sync timed out"))?
            .map_err(|e| err(format!("initial sync failed: {e:#}")))?;

        // Persist the shared snapshot BEFORE creating a mount node in
        // the global tree. If save fails, no mount node has been
        // created — drop the doc and return Err.
        if let Err(e) = self.snapshot_store.save(&shared_tree_id, &shared_arc) {
            self.degraded_bus.emit(ShareDegraded {
                shared_tree_id: shared_tree_id.clone(),
                reason: ShareDegradedReason::SnapshotSaveFailed(format!("{e:#}")),
            });
            return Err(err(format!(
                "initial snapshot save failed after sync; global tree unchanged: {e:#}"
            )));
        }

        // Determine the shared root: the sole root in the freshly imported doc.
        let shared_root = {
            let tree = shared_arc.get_tree(crate::loro_backend::TREE_NAME);
            let roots = tree.roots();
            if roots.len() != 1 {
                return Err(err(format!(
                    "expected exactly one root in shared doc, found {}",
                    roots.len()
                )));
            }
            roots[0]
        };

        // Check for an existing mount for this shared tree. Accepting the
        // same ticket twice would create two mount nodes whose descendants
        // can only be parented to one — the second mount would be empty.
        // Return the existing mount instead.
        let collab = self.global_doc().await?;
        let no_parent = EntityUri::no_parent().as_str().to_string();
        // Set when the mount is parented under the "Shared with me" recipient
        // root (H7) — drives the post-lock SQL projection of that root row.
        let mut attached_to_shared_with_me = false;
        let (mount_stable_id, mount_parent_uri) = {
            let doc_arc = collab.doc();
            let doc = &*doc_arc;

            if let Some((existing_tid, existing_uri)) =
                find_mount_by_shared_tree_id(doc, &shared_tree_id)
            {
                // Idempotent re-accept: the mount already exists. Recover its
                // (already page-resolved) SQL parent from its Loro placement.
                let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                // Fail loud identically to the fresh-accept path below: a mount
                // parented under a node with no STABLE_ID is a corrupt tree, not
                // a reason to silently relocate the mount to top-level.
                let parent_uri_existing = match parent_as_option(doc, existing_tid) {
                    Some(ptid) => read_stable_id(&tree, ptid)
                        .map(|s| block_uri_from_bare(&s))
                        .ok_or_else(|| {
                            err(format!(
                                "existing mount's parent node {ptid:?} has no STABLE_ID; cannot \
                                 resolve the mount's SQL parent"
                            ))
                        })?,
                    None => no_parent.clone(),
                };
                (existing_uri, parent_uri_existing)
            } else {
                let new_id = format!("block:{}", Uuid::new_v4());
                let parent_tid = find_tree_id_by_stable_id(doc, &parent_uri)
                    .ok_or_else(|| err(format!("parent block {parent_id} not found")))?;
                // Amendment A: the mount is a Page, so bubble the accept target
                // to its nearest page ancestor — a no-op when the user targeted
                // a page (the common case). The SAME resolved parent lands in
                // both the Loro tree and the SQL row, keeping the stores
                // aligned.
                //
                // H7 (ADR 0028): when the target has NO page ancestor, the mount
                // would orphan at `no_parent` — present in SQL but invisible in
                // the UI (dogfood N3, 2026-07-20). Attach it under the dedicated
                // "Shared with me" recipient root instead, so accepted shares are
                // always reachable and rendered.
                let page_parent_tid = match nearest_page_ancestor_tid(doc, parent_tid)? {
                    Some(p) => Some(p),
                    None => {
                        attached_to_shared_with_me = true;
                        Some(ensure_shared_with_me_root_node(doc)?)
                    }
                };

                let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                let mount = shared_tree::create_mount_node(
                    &tree,
                    page_parent_tid,
                    &shared_tree_id,
                    shared_root,
                )
                .map_err(|e| err(format!("create mount node: {e:#}")))?;
                set_stable_id(doc, mount, &new_id)
                    .map_err(|e| err(format!("set mount stable_id: {e:#}")))?;
                doc.commit();
                let mount_parent_uri = match page_parent_tid {
                    Some(ptid) => read_stable_id(&tree, ptid)
                        .map(|s| block_uri_from_bare(&s))
                        .ok_or_else(|| err(format!("page ancestor {ptid:?} has no STABLE_ID")))?,
                    None => no_parent.clone(),
                };
                (new_id, mount_parent_uri)
            }
        };

        // Flush the global doc so the mount node is durable.
        if let Err(e) = self.store.read().await.save_all().await {
            self.degraded_bus.emit(ShareDegraded {
                shared_tree_id: shared_tree_id.clone(),
                reason: ShareDegradedReason::SnapshotSaveFailed(format!(
                    "global doc save_all failed after accept: {e:#}"
                )),
            });
            return Err(err(format!("global doc save_all failed: {e:#}")));
        }

        // H7: if the mount was parented under the "Shared with me" recipient
        // root, project that root row FIRST so the mount's SQL parent resolves
        // to a rendered page (the UI reads SQL). Idempotent (INSERT OR IGNORE).
        if attached_to_shared_with_me {
            self.project_shared_with_me_root_to_sql().await?;
        }

        // Project the mount node as a Page Block row so the UI (SQL matviews)
        // and the org write-back (name_chain terminates at the mount page) both
        // resolve the shared content. D3: adopt the shared root's title when it
        // is a page (the mount page IS that page); otherwise synthetic
        // container. Identifiable via the `share-role=mount` property.
        let (root_is_page, root_title) = shared_root_summary(&shared_arc)?;
        let mount_title = if root_is_page {
            root_title
        } else {
            format!("Shared tree ({shared_tree_id})")
        };
        self.project_mount_to_sql(
            &mount_stable_id,
            &mount_parent_uri,
            &shared_tree_id,
            &mount_title,
        )
        .await?;

        self.project_descendants_to_sql(&shared_arc, &mount_stable_id, &shared_tree_id)
            .await?;

        self.manager
            .register_arc(shared_tree_id.clone(), shared_arc.clone());

        // Advertiser was started before the initial sync (see top of
        // this function) so the dialer endpoint is long-lived and the
        // remote peer's accept-loop callback records an addr that's
        // still valid after the sync completes.

        self.attach_sync_worker(shared_tree_id.clone(), shared_arc.clone())
            .await;
        self.attach_projection_worker(
            shared_tree_id.clone(),
            shared_arc.clone(),
            mount_stable_id.clone(),
        )
        .await?;
        self.attach_save_worker(shared_tree_id.clone(), shared_arc)
            .await;

        let response = serde_json::json!({
            "mount_block_id": mount_stable_id,
            "shared_tree_id": shared_tree_id,
        });
        Ok(
            OperationResult::irreversible(vec![])
                .with_response(Value::String(response.to_string())),
        )
    }

    async fn gc_orphans(&self) -> Result<OperationResult> {
        // Enumerate mount nodes in the global tree — the source of
        // truth for "which shared trees are still in use".
        let collab = self.global_doc().await?;
        let known: std::collections::HashSet<String> = {
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            tree.get_nodes(false)
                .into_iter()
                .filter(|n| !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist))
                .filter(|n| shared_tree::is_mount_node(&tree, n.id))
                .filter_map(|n| shared_tree::read_mount_info(&tree, n.id))
                .map(|m| m.shared_tree_id)
                .collect()
        };

        let on_disk = self
            .snapshot_store
            .list_snapshots()
            .map_err(|e| err(format!("list_snapshots: {e:#}")))?;

        let mut deleted: Vec<String> = Vec::new();
        for id in on_disk {
            if known.contains(&id) {
                continue;
            }
            self.snapshot_store
                .delete_snapshot(&id)
                .map_err(|e| err(format!("delete_snapshot({id}): {e:#}")))?;
            deleted.push(id);
        }

        let response = serde_json::json!({ "deleted": deleted });
        Ok(
            OperationResult::irreversible(vec![])
                .with_response(Value::String(response.to_string())),
        )
    }

    async fn unshare(&self, mount_block_id: &str) -> Result<OperationResult> {
        let mount_uri = EntityUri::parse(mount_block_id)
            .map_err(|e| err(format!("invalid mount block URI {mount_block_id:?}: {e:#}")))?;
        if !mount_uri.is_block() {
            return Err(err(format!(
                "unshare expects a `block:` URI, got scheme {:?} (full URI: {mount_block_id:?})",
                mount_uri.scheme()
            )));
        }

        // (1) Resolve the shared_tree_id + mount TreeID from the mount block id.
        // Loud error if the block is not a mount node.
        let collab = self.global_doc().await?;
        let (mount_tid, shared_tree_id) = {
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let tid = find_tree_id_by_stable_id(doc, &mount_uri).ok_or_else(|| {
                err(format!(
                    "mount block {mount_block_id} not found in Loro tree"
                ))
            })?;
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            let info = shared_tree::read_mount_info(&tree, tid).ok_or_else(|| {
                err(format!(
                    "block {mount_block_id} is not a mount node; unshare only applies to mounts"
                ))
            })?;
            (tid, info.shared_tree_id)
        };

        // (2) Drop the per-share workers FIRST. Dropping each handle aborts its
        // task, so no save/sync/projection worker can resurrect the snapshot
        // (or re-project SQL) after we tear the rest down — closes the
        // gc_orphans-style resurrection race.
        self.save_workers.write().await.remove(&shared_tree_id);
        self.sync_workers.write().await.remove(&shared_tree_id);
        self.projection_workers
            .write()
            .await
            .remove(&shared_tree_id);

        // (3) Close the advertiser endpoint + stop advertising.
        self.advertiser
            .drop_share(&shared_tree_id)
            .await
            .map_err(|e| err(format!("drop_share({shared_tree_id}): {e:#}")))?;

        // (4) Unregister the shared doc from the manager. Keep the handle so we
        // can enumerate the descendant block ids for SQL row deletion below.
        let shared_doc = self.manager.remove(&shared_tree_id);

        // (5) Delete the mount node from the global tree so the block leaves the
        // UI, then flush to disk. The global Loro→SQL projection will delete the
        // mount row on its next diff (one-time; base advances). We ALSO delete
        // the mount + projected descendant rows explicitly so the disappearance
        // is immediate and deterministic — the descendant rows live only in SQL
        // (they were never in the global doc), so the global projection never
        // touches them.
        {
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            // Name the mount's root containers BEFORE the delete: the delete
            // cascades and a gone node no longer names its roots. Without the
            // purge they outlive it, holding their content in the global doc's
            // state and so in every export of it.
            let mount_roots = crate::deleted_container_purge::subtree_roots(&tree, mount_tid)
                .map_err(|e| err(format!("collect mount node roots to purge: {e:#}")))?;
            tree.delete(mount_tid)
                .map_err(|e| err(format!("delete mount node {mount_block_id}: {e:#}")))?;
            crate::deleted_container_purge::purge_roots(doc, &mount_roots)
                .map_err(|e| err(format!("purge mount node containers: {e:#}")))?;
            doc.commit();
        }
        self.store
            .read()
            .await
            .save_all()
            .await
            .map_err(|e| err(format!("global doc save_all after unshare: {e:#}")))?;

        if let Some(sql_ops) = self.sql_ops.as_ref() {
            let entity = EntityName::new("block");
            if let Some(doc) = shared_doc.as_ref() {
                for id in crate::loro_backend::snapshot_blocks_from_doc(doc).into_keys() {
                    let mut params = StorageEntity::new();
                    params.insert("id".into(), Value::String(id.clone()));
                    sql_ops
                        .execute_operation(&entity, "delete", params)
                        .await
                        .map_err(|e| {
                            err(format!("unshare: delete descendant row {id} from SQL: {e}"))
                        })?;
                }
            }
            let mut params = StorageEntity::new();
            params.insert("id".into(), Value::String(mount_block_id.to_string()));
            sql_ops
                .execute_operation(&entity, "delete", params)
                .await
                .map_err(|e| err(format!("unshare: delete mount row from SQL: {e}")))?;
        }

        // (6) Delete the on-disk snapshot — now safe, all workers are gone.
        self.snapshot_store
            .delete_snapshot(&shared_tree_id)
            .map_err(|e| err(format!("delete_snapshot({shared_tree_id}): {e:#}")))?;

        let response = serde_json::json!({
            "shared_tree_id": shared_tree_id,
            "mount_block_id": mount_block_id,
        });
        Ok(
            OperationResult::irreversible(vec![])
                .with_response(Value::String(response.to_string())),
        )
    }
}

#[async_trait]
impl OperationProvider for LoroShareBackend {
    fn operations(&self) -> Vec<OperationDescriptor> {
        __operations_subtree_share_operations::subtree_share_operations(
            TREE_ENTITY,
            TREE_ENTITY,
            TREE_ENTITY,
            "id",
        )
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: StorageEntity,
    ) -> Result<OperationResult> {
        if entity_name != TREE_ENTITY {
            return Err(err(format!(
                "LoroShareBackend expects entity '{TREE_ENTITY}', got '{entity_name}'"
            )));
        }

        let result = __operations_subtree_share_operations::dispatch_operation::<_, ()>(
            self, op_name, &params,
        )
        .await?;

        Ok(OperationResult {
            changes: result.changes,
            undo: match result.undo {
                UndoAction::Undo(mut op) => {
                    op.entity_name = entity_name.clone();
                    UndoAction::Undo(op)
                }
                other => other,
            },
            delivery: holon_core::Delivery::Proven,
            response: result.response,
            follow_ups: result.follow_ups,
        })
    }
}

/// Walk the global Loro doc's mount nodes and rehydrate each share:
/// load the snapshot from disk, register it with the manager, start
/// advertising (tolerating "already advertising"), attach a save
/// worker. Returns the count of successfully rehydrated shares.
///
/// Snapshots that fail to load are already quarantined + logged by
/// [`SharedSnapshotStore::load`]; we just skip them. Orphan snapshot
/// files (no matching mount node in the global tree) are logged at
/// `info!` — the global tree is authoritative, not the filesystem.
pub async fn rehydrate_shared_trees(
    backend: &LoroShareBackend,
    global_doc: &LoroDoc,
) -> Result<usize> {
    // Clean up any `.tmp` files from a previous crashed write before
    // any reads can see them.
    if let Err(e) = backend.snapshot_store.sweep_stale_tmps() {
        warn!("[share] sweep_stale_tmps failed: {e:#}");
    }

    // Enumerate mount nodes in the global tree. Also capture each
    // mount's STABLE_ID and its parent's STABLE_ID while we have the
    // doc lock — both are needed to project the mount row into SQL
    // below (block table keys blocks by `block:<uuid>` URI).
    let mount_records: Vec<MountRehydrationRecord> = {
        let tree = global_doc.get_tree(crate::loro_backend::TREE_NAME);
        let mut out = Vec::new();
        for node in tree.get_nodes(false) {
            let parent_tid = match node.parent {
                TreeParentId::Node(p) => Some(p),
                TreeParentId::Root => None,
                TreeParentId::Deleted | TreeParentId::Unexist => continue,
            };
            if !shared_tree::is_mount_node(&tree, node.id) {
                continue;
            }
            let Some(info) = shared_tree::read_mount_info(&tree, node.id) else {
                continue;
            };
            let mount_stable_id = read_stable_id(&tree, node.id);
            let parent_stable_id = parent_tid.and_then(|pid| read_stable_id(&tree, pid));
            out.push(MountRehydrationRecord {
                info,
                mount_stable_id,
                parent_stable_id,
            });
        }
        out
    };

    // Diagnostic: report orphan snapshots (file on disk but no mount
    // node pointing at it). The global tree is the source of truth.
    if let Ok(on_disk) = backend.snapshot_store.list_snapshots() {
        let known: std::collections::HashSet<&str> = mount_records
            .iter()
            .map(|m| m.info.shared_tree_id.as_str())
            .collect();
        for id in on_disk {
            if !known.contains(id.as_str()) {
                tracing::info!(
                    shared_tree_id = %id,
                    "[share] orphan snapshot on disk — no mount node in global tree"
                );
            }
        }
    }

    let mut rehydrated = 0usize;
    for record in mount_records {
        let info = record.info;
        let shared_tree_id = info.shared_tree_id.clone();
        let doc = match backend.snapshot_store.load(&shared_tree_id) {
            Ok(doc) => doc,
            Err(e) => {
                // `load` already emitted SnapshotLoadFailed + quarantined
                // the file. Skip this share entirely — user must re-accept.
                warn!(
                    shared_tree_id = %shared_tree_id,
                    error = %e,
                    "[share] skipping unrehydratable share"
                );
                continue;
            }
        };

        // Bump the generation on every rehydrate so post-restart edits
        // author under a FRESH peer-id: even a stale loaded snapshot then
        // cannot cause `(peer_id, counter)` reuse — see `share_peer_id`.
        let generation = match backend.snapshot_store.next_generation(&shared_tree_id) {
            Ok(g) => g,
            Err(e) => {
                warn!(
                    shared_tree_id = %shared_tree_id,
                    error = %e,
                    "[share] bump peer-id generation during rehydrate failed"
                );
                backend.degraded_bus.emit(ShareDegraded {
                    shared_tree_id: shared_tree_id.clone(),
                    reason: ShareDegradedReason::RehydrationFailed(format!(
                        "next_generation: {e:#}"
                    )),
                });
                continue;
            }
        };
        let peer_id = stable_peer_id(&backend.device_key, &shared_tree_id, generation);
        if let Err(e) = doc.set_peer_id(peer_id) {
            warn!(
                shared_tree_id = %shared_tree_id,
                error = %e,
                "[share] set_peer_id during rehydrate failed"
            );
            backend.degraded_bus.emit(ShareDegraded {
                shared_tree_id: shared_tree_id.clone(),
                reason: ShareDegradedReason::RehydrationFailed(format!("set_peer_id: {e:#}")),
            });
            continue;
        }

        let arc = Arc::new(doc);
        backend
            .manager
            .register_arc(shared_tree_id.clone(), arc.clone());

        // Load the sidecar peer list, if present. Missing sidecar is
        // normal (fresh share pre-autopersist, or the file was never
        // written). Malformed sidecar is a degraded signal but we
        // still rehydrate — the share is still usable, just without
        // any remembered peers until a fresh connection repopulates
        // the list.
        match backend.snapshot_store.load_peers(&shared_tree_id) {
            Ok(peers) if !peers.is_empty() => {
                let mut guard = backend.known_peers.write().await;
                guard.insert(shared_tree_id.clone(), peers);
            }
            Ok(_) => {}
            Err(e) => {
                warn!(
                    shared_tree_id = %shared_tree_id,
                    error = %e,
                    "[share] load_peers during rehydrate failed"
                );
                backend.degraded_bus.emit(ShareDegraded {
                    shared_tree_id: shared_tree_id.clone(),
                    reason: ShareDegradedReason::RehydrationFailed(format!("load_peers: {e:#}")),
                });
            }
        }

        // Start advertising. "Already advertising" is success (e.g.,
        // two rehydration paths got wired up). Other errors are
        // degraded-mode but non-fatal — the share is still in the
        // registry and can be pulled from.
        match backend
            .start_advertising_stable(&shared_tree_id, arc.clone())
            .await
        {
            Ok(_) => {}
            Err(e) if is_already_advertising(&e) => {}
            Err(e) => {
                warn!(
                    shared_tree_id = %shared_tree_id,
                    error = %e,
                    "[share] advertiser start_share failed during rehydrate"
                );
                backend.degraded_bus.emit(ShareDegraded {
                    shared_tree_id: shared_tree_id.clone(),
                    reason: ShareDegradedReason::RehydrationFailed(format!("advertiser: {e:#}")),
                });
                // Intentionally continue — the share is usable for
                // pulls even without advertising.
            }
        }

        backend
            .attach_save_worker(shared_tree_id.clone(), arc.clone())
            .await;
        backend
            .attach_sync_worker(shared_tree_id.clone(), arc.clone())
            .await;

        // Kick an initial sync to every known peer so (a) our fresh
        // endpoint addr gets registered on the other side's
        // advertiser and (b) we pull any edits that landed while we
        // were offline. Spawned non-blocking so rehydrate can process
        // multiple shares concurrently. Retries with backoff: the
        // peer may itself be mid-restart (its endpoint torn down but
        // not yet rebound), and a failed kick would otherwise leave
        // both sides holding stale addrs with nothing to repair them.
        let backend_for_kick = backend.weak_self();
        let kick_id = shared_tree_id.clone();
        let had_peers = {
            let guard = backend.known_peers.read().await;
            guard.get(&shared_tree_id).is_some_and(|p| !p.is_empty())
        };
        if had_peers {
            tokio::spawn(async move {
                let mut delay = Duration::from_secs(1);
                for attempt in 1u32..=3 {
                    let Some(strong) = backend_for_kick.upgrade() else {
                        return;
                    };
                    match strong.sync_with_peers(&kick_id).await {
                        Ok(n) if n > 0 => {
                            tracing::debug!(
                                shared_tree_id = %kick_id,
                                peers_synced = %n,
                                attempt = attempt,
                                "[share] rehydrate kick-sync complete"
                            );
                            return;
                        }
                        Ok(_) => {}
                        Err(e) => warn!(
                            shared_tree_id = %kick_id,
                            error = %e,
                            attempt = attempt,
                            "[share] rehydrate kick-sync failed"
                        ),
                    }
                    drop(strong);
                    if attempt < 3 {
                        tokio::time::sleep(delay).await;
                        delay *= 2;
                    }
                }
                warn!(
                    shared_tree_id = %kick_id,
                    "[share] rehydrate kick-sync reached no peer after 3 attempts — \
                     cross-peer sync resumes when a peer dials us or a local edit retries"
                );
            });
        }

        // Re-project the mount row into SQL. `INSERT OR IGNORE` on the
        // block table makes this safe across restarts — if the row is
        // already there, nothing happens; if it was somehow lost (e.g.
        // DB was wiped while the Loro snapshot survived), this repairs
        // it. Requires both the mount's own stable id and its parent's
        // stable id; either missing skips the projection with a warn!.
        match (
            record.mount_stable_id.as_deref(),
            record.parent_stable_id.as_deref(),
        ) {
            (Some(mount_bare), Some(parent_bare)) => {
                let mount_uri = block_uri_from_bare(mount_bare);
                let parent_uri = block_uri_from_bare(parent_bare);
                // D3: re-adopt the shared root's title on rehydrate so the
                // mount page keeps its identity across restarts. The mount's
                // parent was page-resolved when it was first created, so it is
                // reprojected as-is.
                let title = match shared_root_summary(&arc) {
                    Ok((true, root_title)) => root_title,
                    Ok((false, _)) => format!("Shared tree ({shared_tree_id})"),
                    Err(e) => {
                        // Disclosed degraded mode (mirrors the projection-error
                        // warns below — one unreadable share must not abort
                        // rehydration of the others): keep the mount visible
                        // under its synthetic id, surfaced loudly.
                        warn!(
                            shared_tree_id = %shared_tree_id,
                            error = %e,
                            "[share] shared_root_summary during rehydrate failed; \
                             projecting mount with synthetic placeholder title"
                        );
                        format!("Shared tree ({shared_tree_id})")
                    }
                };
                if let Err(e) = backend
                    .project_mount_to_sql(&mount_uri, &parent_uri, &shared_tree_id, &title)
                    .await
                {
                    warn!(
                        shared_tree_id = %shared_tree_id,
                        error = %e,
                        "[share] project_mount_to_sql during rehydrate failed"
                    );
                }
                if let Err(e) = backend
                    .project_descendants_to_sql(&arc, &mount_uri, &shared_tree_id)
                    .await
                {
                    warn!(
                        shared_tree_id = %shared_tree_id,
                        error = %e,
                        "[share] project_descendants_to_sql during rehydrate failed"
                    );
                }
                if let Err(e) = backend
                    .attach_projection_worker(shared_tree_id.clone(), arc.clone(), mount_uri)
                    .await
                {
                    warn!(
                        shared_tree_id = %shared_tree_id,
                        error = %e,
                        "[share] attach_projection_worker during rehydrate failed"
                    );
                }
            }
            _ => {
                warn!(
                    shared_tree_id = %shared_tree_id,
                    mount_stable_id = ?record.mount_stable_id,
                    parent_stable_id = ?record.parent_stable_id,
                    "[share] skipping mount row projection — missing stable id(s)"
                );
            }
        }

        rehydrated += 1;
    }

    Ok(rehydrated)
}

/// Internal record bundling a `MountInfo` with the stable ids needed to
/// project the mount row into SQL. Lives in this module rather than
/// `shared_tree.rs` because the consumer (rehydrate) is the only caller.
struct MountRehydrationRecord {
    info: shared_tree::MountInfo,
    mount_stable_id: Option<String>,
    parent_stable_id: Option<String>,
}

/// Read the `STABLE_ID` metadata from a tree node. Returns `None` when
/// the node has no metadata, the key is missing, or the value isn't a
/// string. The returned id is whatever the meta holds — callers should
/// not assume presence or absence of a `block:` scheme prefix (see
/// [`block_uri_from_bare`] for the normalization).
fn read_stable_id(tree: &loro::LoroTree, tid: TreeID) -> Option<String> {
    let meta = tree.get_meta(tid).ok()?; // ALLOW(ok): Option chain — missing meta means no stable id
    let v = meta.get(STABLE_ID)?;
    match v {
        ValueOrContainer::Value(val) => val.as_string().map(|s| s.to_string()),
        _ => None,
    }
}

/// Canonicalize a stable id read from Loro metadata into a full
/// `block:<uuid>` URI. Some code paths store the bare UUID; others —
/// notably `accept_shared_subtree` itself — store the pre-prefixed
/// form. Strip `block:` if present, then re-prefix so the SQL row's
/// `id` column matches the URI shape used everywhere else.
fn block_uri_from_bare(stored: &str) -> String {
    let bare = stored.strip_prefix("block:").unwrap_or(stored);
    format!("block:{bare}")
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::loro_document_store::LoroDocumentStore;

    fn make_backend() -> (Arc<LoroShareBackend>, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(
            dir.path().to_path_buf(),
        )));
        let bus = Arc::new(DegradedSignalBus::new());
        let snapshot_store = Arc::new(SharedSnapshotStore::new(
            dir.path().to_path_buf(),
            bus.clone(),
        ));
        let manager = Arc::new(SharedTreeSyncManager::new());
        // Use the persistent key-from-disk path so a `drop+re-make`
        // simulates a real process restart (same device identity).
        let key = crate::device_key_store::load_or_create_device_key(dir.path()).unwrap();
        let advertiser = Arc::new(IrohAdvertiser::new_with_key(key.clone()));
        (
            LoroShareBackend::new(store, snapshot_store, manager, advertiser, bus, key),
            dir,
        )
    }

    /// F1.1 regression: a node WITHHELD from the settled snapshot (here:
    /// `STABLE_ID` transiently unreadable, the same shape as an in-flight
    /// create/move whose meta hasn't landed) must NOT diff as a real SQL
    /// DELETE on the share-projection path. The ungated diff DOES see a
    /// delete — the gate in `share_diff_ops` is what withholds it.
    #[test]
    fn withheld_node_emits_no_delete_on_share_path() {
        use crate::loro_backend::TREE_NAME;
        use crate::loro_backend::snapshot_blocks_from_doc;
        use crate::loro_sync_controller::diff_snapshots_to_ops;

        let doc = loro::LoroDoc::new();
        let tree = doc.get_tree(TREE_NAME);
        let node = tree.create(None).unwrap();
        tree.get_meta(node)
            .unwrap()
            .insert(STABLE_ID, "11111111-1111-1111-1111-111111111111")
            .unwrap();
        doc.commit();
        let watermark = doc.oplog_frontiers();

        // Mid-mutation shape: node alive, meta momentarily incomplete.
        tree.get_meta(node).unwrap().delete(STABLE_ID).unwrap();
        doc.commit();

        let fork = doc.fork_at(&watermark).unwrap();
        let before = snapshot_blocks_from_doc(&fork);
        assert_eq!(before.len(), 1, "node must be present in the base snapshot");

        // Precondition: the ungated diff (the pre-fix worker behaviour)
        // really does classify the withheld node as a delete.
        let naive_after = snapshot_blocks_from_doc(&doc);
        let naive = diff_snapshots_to_ops(&before, &naive_after);
        assert!(
            naive.iter().any(|(name, _)| name == "delete"),
            "precondition: ungated diff must see a delete, got {naive:?}"
        );

        let (ops, settled) = share_diff_ops(&doc, &before, &|_| {}, "test-tree");
        assert!(
            !settled,
            "snapshot with unreadable node meta must be unsettled"
        );
        assert!(
            !ops.iter().any(|(name, _)| name == "delete"),
            "withheld node must not become a SQL DELETE on the share path: {ops:?}"
        );
    }

    /// Companion to the withhold test: a LEGITIMATE delete (node parented to
    /// Deleted) keeps the snapshot settled and its DELETE op still flows.
    #[test]
    fn legitimate_delete_still_flows_on_share_path() {
        use crate::loro_backend::TREE_NAME;
        use crate::loro_backend::snapshot_blocks_from_doc;

        let doc = loro::LoroDoc::new();
        let tree = doc.get_tree(TREE_NAME);
        let node = tree.create(None).unwrap();
        tree.get_meta(node)
            .unwrap()
            .insert(STABLE_ID, "22222222-2222-2222-2222-222222222222")
            .unwrap();
        doc.commit();
        let watermark = doc.oplog_frontiers();

        tree.delete(node).unwrap();
        doc.commit();

        let fork = doc.fork_at(&watermark).unwrap();
        let before = snapshot_blocks_from_doc(&fork);
        assert_eq!(before.len(), 1);

        let (ops, settled) = share_diff_ops(&doc, &before, &|_| {}, "test-tree");
        assert!(settled, "a genuine delete must not unsettle the snapshot");
        assert!(
            ops.iter().any(|(name, _)| name == "delete"),
            "a genuine delete must still project: {ops:?}"
        );
    }

    /// Regression (ADR 0028 §H4 / loro fork W2): the live share-projection
    /// worker forks the RECIPIENT's doc at a watermark to compute incremental
    /// diffs (`fork_at(&last)` at loro_share_backend.rs:362). A recipient's
    /// shared subtree is a *shallow* snapshot imported into a fresh doc (see
    /// shared_tree.rs `HistoryRetention::None`), and the watermark is
    /// initialised to that doc's `oplog_frontiers()` == the shallow root
    /// (loro_share_backend.rs:323), then only ever advanced forward. So every
    /// `fork_at` targets a frontier at or after the shallow root.
    ///
    /// Upstream loro (through 1.13.7) rejected `fork_at` on ANY shallow doc
    /// with `NotImplemented("fork_at on shallow docs")`, which killed live
    /// share-sync projection. The Holon loro fork implements the
    /// at/after-shallow-root case; this exercises the exact recipient shape.
    #[test]
    fn fork_at_watermark_on_shallow_recipient_doc() {
        use loro::ExportMode;

        use crate::loro_backend::TREE_NAME;
        use crate::loro_backend::configure_text_styles;
        use crate::loro_backend::snapshot_blocks_from_doc;

        // Owner side: a tree with one shared node.
        let owner = loro::LoroDoc::new();
        let tree = owner.get_tree(TREE_NAME);
        let node = tree.create(None).unwrap();
        tree.get_meta(node)
            .unwrap()
            .insert(STABLE_ID, "33333333-3333-3333-3333-333333333333")
            .unwrap();
        owner.commit();

        // Recipient side: state-only (shallow) snapshot imported into a fresh
        // doc, mirroring `HistoryRetention::None` in shared_tree.rs.
        let snapshot = owner
            .export(ExportMode::shallow_snapshot(&owner.oplog_frontiers()))
            .unwrap();
        let recipient = loro::LoroDoc::new();
        configure_text_styles(&recipient);
        recipient.set_peer_id(0x5eed).unwrap();
        recipient.import(&snapshot).unwrap();
        assert!(recipient.is_shallow(), "recipient doc must be shallow");

        // Worker watermark == recipient's current frontiers == the shallow root.
        let watermark = recipient.oplog_frontiers();

        // A remote op imports (a second node appears), advancing past the root.
        let tree_r = recipient.get_tree(TREE_NAME);
        let node2 = tree_r.create(None).unwrap();
        tree_r
            .get_meta(node2)
            .unwrap()
            .insert(STABLE_ID, "44444444-4444-4444-4444-444444444444")
            .unwrap();
        recipient.commit();

        // THE REGRESSION: forking the shallow recipient doc at the watermark.
        // Before the loro fork this returned NotImplemented and the worker died.
        let fork = recipient
            .fork_at(&watermark)
            .expect("fork_at at/after the shallow root must succeed on a shallow recipient doc");
        let before = snapshot_blocks_from_doc(&fork);
        assert_eq!(
            before.len(),
            1,
            "fork at the watermark = the shallow-root state (one node)"
        );

        // The live doc has advanced to two nodes; the incremental diff is real.
        let after = snapshot_blocks_from_doc(&recipient);
        assert_eq!(after.len(), 2, "recipient advanced to two nodes");

        // Forking at the advanced frontier also works and reflects both nodes.
        let fork_latest = recipient.fork_at(&recipient.oplog_frontiers()).unwrap();
        assert_eq!(snapshot_blocks_from_doc(&fork_latest).len(), 2);
    }

    #[test]
    fn parse_retention_none_is_accepted() {
        assert!(matches!(
            parse_retention("none").ok().unwrap(),
            HistoryRetention::None
        ));
    }

    #[test]
    fn parse_retention_full_is_rejected_as_leaky() {
        // "full" is disabled at the boundary because it leaks the whole vault's
        // history to the recipient (docs/Reference/SUBTREE_SHARING.md B1).
        let err = parse_retention("full").err().unwrap();
        let msg = format!("{err}");
        assert!(msg.contains("disabled"), "unexpected error: {msg}");
        assert!(msg.contains("leak"), "unexpected error: {msg}");
    }

    #[test]
    fn parse_retention_since_is_rejected() {
        let err = parse_retention("since").err().unwrap();
        assert!(format!("{err}").contains("not yet supported"));
    }

    #[test]
    fn parse_retention_unknown_is_rejected() {
        let err = parse_retention("garbage").err().unwrap();
        assert!(format!("{err}").contains("unknown retention"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accept_rejects_malformed_ticket() {
        let (backend, _dir) = make_backend();
        let err = backend
            .accept_shared_subtree("block:parent-uuid", "!!!not-a-ticket!!!".into())
            .await
            .unwrap_err();
        assert!(format!("{err}").contains("decode ticket"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn accept_rejects_non_block_parent_uri() {
        // Parse-don't-validate boundary: `accept_shared_subtree` now requires
        // a well-formed `block:` URI for the parent. Hit the scheme gate.
        let (backend, _dir) = make_backend();
        let err = backend
            .accept_shared_subtree("file:something", "ignored".into())
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("expects a `block:` URI"),
            "expected a scheme-gate error, got: {err}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn share_rejects_unknown_block() {
        let (backend, _dir) = make_backend();
        // No block with this stable_id exists in the empty store, so the
        // downstream Loro lookup should fail.
        let err = backend
            .share_subtree("block:00000000-0000-0000-0000-000000000000", "none".into())
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("not found") || msg.contains("get_global_doc"),
            "expected a lookup error, got: {msg}"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn share_rejects_non_block_uri() {
        // Parse-don't-validate boundary: `share_subtree` requires a `block:` URI.
        let (backend, _dir) = make_backend();
        let err = backend
            .share_subtree("file:foo", "none".into())
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("expects a `block:` URI"),
            "expected a scheme-gate error, got: {err}"
        );
    }

    /// N4 (dogfood 2026-07-20): sharing the root layout block must be rejected
    /// loudly — it wraps the whole UI under a mount and can destroy the vault.
    /// RED (pre-guard): `share_subtree` accepted it and returned a ticket.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn share_rejects_root_layout_block() {
        let (backend, _dir) = make_backend();
        let err = backend
            .share_subtree(holon_api::ROOT_LAYOUT_BLOCK_ID, "none".into())
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("root layout block"),
            "expected a structural-share rejection, got: {err}"
        );
    }

    /// N4: the default-document root (owns the bundled layout) is equally
    /// structural and must be rejected.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn share_rejects_default_doc_root() {
        let (backend, _dir) = make_backend();
        let err = backend
            .share_subtree(holon_api::DEFAULT_DOC_BLOCK_ID, "none".into())
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("default-document root"),
            "expected a structural-share rejection, got: {err}"
        );
    }

    /// Seed a block into the global doc with a given stable_id and text
    /// content under an existing parent (or no parent for a root).
    async fn seed_block(
        backend: &LoroShareBackend,
        stable_id: &str,
        parent_stable_id: Option<&str>,
        content: &str,
    ) {
        let collab = backend.global_doc().await.unwrap();
        let doc_arc = collab.doc();
        let doc = &*doc_arc;
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let parent_tid = parent_stable_id.map(|pid| {
            let parent_uri = EntityUri::block(pid);
            find_tree_id_by_stable_id(doc, &parent_uri)
                .unwrap_or_else(|| panic!("parent {pid} not found"))
        });
        let node = tree.create(parent_tid).unwrap();
        let meta = tree.get_meta(node).unwrap();
        meta.insert(STABLE_ID, loro::LoroValue::from(stable_id))
            .unwrap();
        let text: loro::LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
        text.insert(0, content).unwrap();
        doc.commit();
    }

    /// Like [`seed_block`], but tags the node as a `Page` (writes the `tags`
    /// meta the SQL projection + `node_is_page` read). Used to seed realistic
    /// page topologies for the D3 adopt-and-collapse tests.
    async fn seed_page(
        backend: &LoroShareBackend,
        stable_id: &str,
        parent_stable_id: Option<&str>,
        content: &str,
    ) {
        seed_block(backend, stable_id, parent_stable_id, content).await;
        let collab = backend.global_doc().await.unwrap();
        let doc_arc = collab.doc();
        let doc = &*doc_arc;
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let uri = EntityUri::block(stable_id);
        let tid = find_tree_id_by_stable_id(doc, &uri).unwrap();
        let meta = tree.get_meta(tid).unwrap();
        let tags_json = serde_json::to_string(&[holon_api::block::PAGE_TAG]).unwrap();
        meta.insert("tags", loro::LoroValue::from(tags_json.as_str()))
            .unwrap();
        doc.commit();
    }

    async fn read_text(backend: &LoroShareBackend, stable_id: &str) -> Option<String> {
        let collab = backend.global_doc().await.unwrap();
        let doc_arc = collab.doc();
        let doc = &*doc_arc;
        let uri = EntityUri::block(stable_id);
        let tid = find_tree_id_by_stable_id(doc, &uri)?;
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let meta = tree.get_meta(tid).ok()?; // ALLOW(ok): Option chain — missing meta means no stable id
        match meta.get("content_raw") {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => {
                Some(t.to_string())
            }
            _ => None,
        }
    }

    /// Full share→accept round-trip through the real iroh transport, using
    /// two independent LoroShareBackends.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn share_accept_round_trip() {
        let (backend_a, _dir_a) = make_backend();
        let (backend_b, _dir_b) = make_backend();

        // Backend A: root → shared_parent → shared_child
        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(
            &backend_a,
            "shared-parent",
            Some("root-a"),
            "Shared heading",
        )
        .await;
        seed_block(
            &backend_a,
            "shared-child",
            Some("shared-parent"),
            "Shared child",
        )
        .await;

        // Backend B: just a root where we'll mount
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let share_response = backend_a
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();
        let ticket_json: serde_json::Value = match share_response.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response type: {other:?}"),
        };
        let ticket = ticket_json["ticket"].as_str().unwrap().to_string();

        let accept_response = backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();
        assert!(accept_response.response.is_some());

        // The shared content should now be visible in backend B's tree
        // (the mount node resolves into the shared doc, whose content we
        // read directly from the shared_tree manager).
        let st_id = ticket_json["shared_tree_id"].as_str().unwrap();
        let b_shared_doc = backend_b.manager.get_doc(st_id).unwrap();
        let b_tree = b_shared_doc.get_tree(crate::loro_backend::TREE_NAME);
        let texts: Vec<String> = b_tree
            .get_nodes(false)
            .iter()
            .filter(|n| !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist))
            // ALLOW(filter_map_ok): test assertion — non-text nodes are intentionally skipped
            .filter_map(|n| {
                let meta = b_tree.get_meta(n.id).ok()?; // ALLOW(ok): same as above
                match meta.get("content_raw") {
                    Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => {
                        Some(t.to_string())
                    }
                    _ => None,
                }
            })
            .collect();
        assert!(
            texts.iter().any(|s| s == "Shared heading"),
            "shared parent content missing on B. Got: {texts:?}"
        );
        assert!(
            texts.iter().any(|s| s == "Shared child"),
            "shared child content missing on B. Got: {texts:?}"
        );

        // Clean up advertiser tasks.
        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
        // Silence the unused helper warning — used in the bidirectional test.
        let _ = read_text(&backend_a, "shared-parent").await;
    }

    /// After share+accept, both sides edit; B pulls from A and sees A's
    /// change. Exercises the full ticket → initial-sync → re-sync path.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn bidirectional_edits_converge() {
        let (backend_a, _dir_a) = make_backend();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(
            &backend_a,
            "shared-parent",
            Some("root-a"),
            "Shared heading",
        )
        .await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let share_response = backend_a
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();
        let ticket_json: serde_json::Value = match share_response.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response type: {other:?}"),
        };
        let ticket = ticket_json["ticket"].as_str().unwrap().to_string();
        let shared_tree_id = ticket_json["shared_tree_id"].as_str().unwrap().to_string();

        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();

        // A appends text to the shared heading.
        {
            let a_doc = backend_a.manager.get_doc(&shared_tree_id).unwrap();
            let tree = a_doc.get_tree(crate::loro_backend::TREE_NAME);
            let root = tree.roots()[0];
            let meta = tree.get_meta(root).unwrap();
            let text = match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t,
                _ => panic!("no content_raw on shared root"),
            };
            let len = text.len_unicode();
            text.insert(len, " [edit from A]").unwrap();
            a_doc.commit();
        }

        // B pulls from A.
        let synced = backend_b.sync_with_peers(&shared_tree_id).await.unwrap();
        assert_eq!(synced, 1, "B should have synced with 1 peer (A)");

        // B's shared doc now reflects A's edit.
        let b_doc = backend_b.manager.get_doc(&shared_tree_id).unwrap();
        let b_tree = b_doc.get_tree(crate::loro_backend::TREE_NAME);
        let b_root = b_tree.roots()[0];
        let meta = b_tree.get_meta(b_root).unwrap();
        let b_text = match meta.get("content_raw") {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t.to_string(),
            _ => panic!("no content_raw on B's shared root"),
        };
        assert!(
            b_text.contains("[edit from A]"),
            "B should see A's edit after pull. Got: {b_text:?}"
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// Read the `content_raw` text of the node with `stable_id` from a specific
    /// shared doc (by `shared_tree_id`), via the backend's manager registry.
    fn shared_text(
        backend: &LoroShareBackend,
        shared_tree_id: &str,
        stable_id: &str,
    ) -> Option<String> {
        let doc = backend.manager.get_doc(shared_tree_id)?;
        let tid = find_tree_id_by_stable_id(&doc, &EntityUri::block(stable_id))?;
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let meta = tree.get_meta(tid).ok()?; // ALLOW(ok): Option chain — missing meta means no stable id
        match meta.get("content_raw") {
            Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => {
                Some(t.to_string())
            }
            _ => None,
        }
    }

    /// B3 (mount-aware write routing): a write to a block INSIDE a shared
    /// subtree must land in the shared doc — not silently no-op on the
    /// global doc, from which the subtree was pruned at share time — and
    /// then sync to the peer.
    ///
    /// Drives the REAL wired write path: a `LoroBackend` over A's global doc
    /// with `with_shared_trees` pointed at the same `SharedTreeSyncManager`
    /// the share machinery uses (the exact wiring `LoroBlockOperations`
    /// receives in DI).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn shared_block_write_routes_to_shared_doc_and_syncs() {
        use crate::loro_backend::LoroBackend;
        use crate::shared_tree::SharedTreeStore;

        let (backend_a, _dir_a) = make_backend();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(
            &backend_a,
            "shared-parent",
            Some("root-a"),
            "Shared heading",
        )
        .await;
        seed_block(
            &backend_a,
            "shared-child",
            Some("shared-parent"),
            "original child",
        )
        .await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let share_response = backend_a
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();
        let ticket_json: serde_json::Value = match share_response.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response type: {other:?}"),
        };
        let ticket = ticket_json["ticket"].as_str().unwrap().to_string();
        let shared_tree_id = ticket_json["shared_tree_id"].as_str().unwrap().to_string();

        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();

        // Precondition: shared-child was pruned from A's GLOBAL doc — a write
        // through the unrouted global path would resolve nothing.
        let a_global = backend_a.global_doc().await.unwrap();
        assert!(
            find_tree_id_by_stable_id(&a_global.doc(), &EntityUri::block("shared-child")).is_none(),
            "shared-child should be absent from A's global tree after the prune"
        );

        // Build the wired write backend exactly as DI does: global doc +
        // the share manager as the SharedTreeStore.
        let manager = backend_a.manager.clone();
        let write_backend = LoroBackend::from_document(a_global.clone())
            .with_shared_trees(manager as Arc<dyn SharedTreeStore>);

        // Route a write to a block that lives only in the shared subtree.
        write_backend
            .update_block_text("block:shared-child", "ROUTED EDIT")
            .await
            .expect("routed shared write must succeed");

        // The edit landed in A's shared doc, not the global doc.
        assert_eq!(
            shared_text(&backend_a, &shared_tree_id, "shared-child").as_deref(),
            Some("ROUTED EDIT"),
            "write must land in A's shared doc"
        );
        // Global doc still holds no shared-child node (nothing was created there).
        assert!(
            find_tree_id_by_stable_id(&a_global.doc(), &EntityUri::block("shared-child")).is_none(),
            "the routed write must NOT resurrect shared-child in the global tree"
        );

        // Peer B pulls from A and sees the routed edit in its shared doc.
        let synced = backend_b.sync_with_peers(&shared_tree_id).await.unwrap();
        assert_eq!(synced, 1, "B should have synced with 1 peer (A)");
        assert_eq!(
            shared_text(&backend_b, &shared_tree_id, "shared-child").as_deref(),
            Some("ROUTED EDIT"),
            "B must see A's routed edit after pull"
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// Share A's `shared-parent` subtree (root-a → shared-parent →
    /// shared-child) and accept it into B. Returns both share backends plus
    /// a `LoroBackend` wired over A's global doc + A's share manager — the
    /// exact DI wiring `LoroBlockOperations` receives. TempDirs are
    /// returned so the on-disk stores outlive the test body.
    #[allow(clippy::type_complexity)]
    async fn share_setup() -> (
        Arc<LoroShareBackend>,
        Arc<LoroShareBackend>,
        crate::loro_backend::LoroBackend,
        String,
        TempDir,
        TempDir,
    ) {
        use crate::loro_backend::LoroBackend;
        use crate::shared_tree::SharedTreeStore;

        let (backend_a, dir_a) = make_backend();
        let (backend_b, dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(
            &backend_a,
            "shared-parent",
            Some("root-a"),
            "Shared heading",
        )
        .await;
        seed_block(
            &backend_a,
            "shared-child",
            Some("shared-parent"),
            "original child",
        )
        .await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let share_response = backend_a
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();
        let ticket_json: serde_json::Value = match share_response.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response type: {other:?}"),
        };
        let ticket = ticket_json["ticket"].as_str().unwrap().to_string();
        let shared_tree_id = ticket_json["shared_tree_id"].as_str().unwrap().to_string();

        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();

        let a_global = backend_a.global_doc().await.unwrap();
        let manager = backend_a.manager.clone();
        let write_backend = LoroBackend::from_document(a_global)
            .with_shared_trees(manager as Arc<dyn SharedTreeStore>);

        (
            backend_a,
            backend_b,
            write_backend,
            shared_tree_id,
            dir_a,
            dir_b,
        )
    }

    /// A read backend wired over B's global doc + B's share manager (the peer's
    /// DI wiring). Used to assert a routed write converged into B's shared doc.
    async fn peer_read_backend(backend_b: &LoroShareBackend) -> crate::loro_backend::LoroBackend {
        use crate::loro_backend::LoroBackend;
        use crate::shared_tree::SharedTreeStore;
        let b_global = backend_b.global_doc().await.unwrap();
        LoroBackend::from_document(b_global)
            .with_shared_trees(backend_b.manager.clone() as Arc<dyn SharedTreeStore>)
    }

    /// Reader parity: `get_block` of a shared block by its stable id resolves
    /// through the shared doc (the id was pruned from A's global tree at share
    /// time) and returns the right content.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn get_block_resolves_shared_stable_id() {
        use holon_api::repository::CoreOperations;
        let (backend_a, backend_b, write_backend, _stid, _da, _db) = share_setup().await;

        let block = write_backend
            .get_block("block:shared-child")
            .await
            .expect("get_block must resolve a shared stable id");
        assert_eq!(block.content, "original child");

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// Group-A routed writers: `update_block_properties` and
    /// `update_block_fields` on a shared block land in the shared doc and
    /// converge to the peer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn routed_property_writes_round_trip_and_sync() {
        use holon_api::repository::CoreOperations;
        let (backend_a, backend_b, write_backend, shared_tree_id, _da, _db) = share_setup().await;

        let mut props = HashMap::new();
        props.insert("TODO".to_string(), Value::String("DONE".to_string()));
        write_backend
            .update_block_properties("block:shared-child", &props)
            .await
            .expect("routed property write must succeed");
        write_backend
            .update_block_fields(
                "block:shared-child",
                &[(
                    "PRIORITY".to_string(),
                    Value::Null,
                    Value::String("A".to_string()),
                )],
            )
            .await
            .expect("routed field write must succeed");

        // Round-trips on A (the child lives ONLY in the shared doc).
        let a_block = write_backend.get_block("block:shared-child").await.unwrap();
        assert_eq!(
            a_block.properties.get("TODO"),
            Some(&Value::String("DONE".to_string()))
        );
        assert_eq!(
            a_block.properties.get("PRIORITY"),
            Some(&Value::String("A".to_string()))
        );

        // Converges to B's shared doc after a pull.
        let synced = backend_b.sync_with_peers(&shared_tree_id).await.unwrap();
        assert_eq!(synced, 1);
        let b_read = peer_read_backend(&backend_b).await;
        let b_block = b_read.get_block("block:shared-child").await.unwrap();
        assert_eq!(
            b_block.properties.get("TODO"),
            Some(&Value::String("DONE".to_string()))
        );
        assert_eq!(
            b_block.properties.get("PRIORITY"),
            Some(&Value::String("A".to_string()))
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// Create-family routing: a child created under a SHARED parent lands in
    /// the shared doc (not the global doc), never pollutes the global
    /// `id_cache`, and syncs to the peer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn create_under_shared_parent_lands_in_shared_doc() {
        use holon_api::repository::CoreOperations;
        let (backend_a, backend_b, write_backend, shared_tree_id, _da, _db) = share_setup().await;

        let child = write_backend
            .create_block(
                EntityUri::block("shared-parent"),
                holon_api::BlockContent::text("born in shared"),
                Some(EntityUri::block("shared-new-child")),
            )
            .await
            .expect("create under shared parent must succeed");
        assert_eq!(child.id, EntityUri::block("shared-new-child"));

        // Landed in A's shared doc.
        assert_eq!(
            shared_text(&backend_a, &shared_tree_id, "shared-new-child").as_deref(),
            Some("born in shared"),
        );
        // NOT in A's global tree.
        let a_global = backend_a.global_doc().await.unwrap();
        assert!(
            find_tree_id_by_stable_id(&a_global.doc(), &EntityUri::block("shared-new-child"))
                .is_none(),
            "shared child must not appear in the global tree"
        );
        // The global id_cache never gained the shared child's id.
        assert!(
            write_backend.peek_id_cache("shared-new-child").is_none(),
            "shared child id must not leak into the global id_cache"
        );

        // Syncs to B.
        let synced = backend_b.sync_with_peers(&shared_tree_id).await.unwrap();
        assert_eq!(synced, 1);
        assert_eq!(
            shared_text(&backend_b, &shared_tree_id, "shared-new-child").as_deref(),
            Some("born in shared"),
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// Delete of a shared block routes to the shared doc.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn delete_shared_block_routes_to_shared_doc() {
        use holon_api::repository::CoreOperations;
        let (backend_a, backend_b, write_backend, shared_tree_id, _da, _db) = share_setup().await;

        assert!(shared_text(&backend_a, &shared_tree_id, "shared-child").is_some());
        write_backend
            .delete_block("block:shared-child")
            .await
            .expect("routed delete must succeed");
        assert!(
            shared_text(&backend_a, &shared_tree_id, "shared-child").is_none(),
            "the shared child must be gone from the shared doc"
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// A cross-doc move (out of a shared subtree into the global doc) rejects
    /// loudly; a same-doc move WITHIN the shared subtree succeeds.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn cross_doc_move_rejects_same_doc_move_succeeds() {
        use holon_api::repository::CoreOperations;
        let (backend_a, backend_b, write_backend, _stid, _da, _db) = share_setup().await;

        // Seed a second child in the shared subtree (create under shared parent).
        write_backend
            .create_block(
                EntityUri::block("shared-parent"),
                holon_api::BlockContent::text("sibling"),
                Some(EntityUri::block("shared-child2")),
            )
            .await
            .unwrap();

        // Cross-doc: shared-child (shared doc) → root-a (global doc) rejects.
        let err = write_backend
            .move_block(
                &EntityUri::block("shared-child"),
                EntityUri::block("root-a"),
                None,
            )
            .await
            .unwrap_err();
        let msg = format!("{err}");
        assert!(
            msg.contains("cross-boundary move"),
            "expected a cross-boundary rejection, got: {msg}"
        );

        // Same-doc: shared-child2 → under shared-child (both in the shared doc)
        // succeeds.
        write_backend
            .move_block(
                &EntityUri::block("shared-child2"),
                EntityUri::block("shared-child"),
                None,
            )
            .await
            .expect("same-doc move within the shared subtree must succeed");

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// A content write to a mount node (the pointer A's global tree holds in
    /// place of the shared subtree) rejects loudly — mounts are not editable.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn write_to_mount_node_rejects() {
        let (backend_a, backend_b, write_backend, _stid, _da, _db) = share_setup().await;

        // Find the mount node A's global tree holds after the share.
        let a_global = backend_a.global_doc().await.unwrap();
        let mount_uri = {
            let doc = a_global.doc();
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            let mount_tid = tree
                .get_nodes(false)
                .iter()
                .find(|n| {
                    !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist)
                        && shared_tree::is_mount_node(&tree, n.id)
                })
                .map(|n| n.id)
                .expect("A's global tree must hold a mount node after share");
            EntityUri::block_from_tree_id(mount_tid.peer, mount_tid.counter).to_string()
        };

        let err = write_backend
            .update_block_text(&mount_uri, "should not land")
            .await
            .unwrap_err();
        assert!(
            format!("{err}").contains("mount node"),
            "expected a mount-node rejection, got: {err}"
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// Drive many commits in rapid succession and assert the save
    /// worker coalesces them into a handful of disk writes. Validates
    /// the `SAVE_DEBOUNCE` window (currently 150 ms).
    #[tokio::test(start_paused = true)]
    async fn save_worker_coalesces_burst() {
        let dir = TempDir::new().unwrap();
        let bus = Arc::new(DegradedSignalBus::new());
        let snapshot_store = Arc::new(SharedSnapshotStore::new(
            dir.path().to_path_buf(),
            bus.clone(),
        ));
        let doc = Arc::new(LoroDoc::new());
        let _worker = spawn_save_worker(
            snapshot_store.clone(),
            bus.clone(),
            "burst".to_string(),
            doc.clone(),
        );

        // Burst of 200 commits under paused tokio time — the debounce
        // sleep won't elapse until we explicitly advance the clock.
        for i in 0..200u32 {
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            let node = tree.create(None::<TreeID>).unwrap();
            tree.get_meta(node).unwrap().insert("n", i as i64).unwrap();
            doc.commit();
            // Yield so the subscribe_root callback + notify wake the
            // worker task; time is still paused, so the debounce sleep
            // inside the worker does not progress.
            tokio::task::yield_now().await;
        }

        // Advance past a single debounce window. The worker wakes once,
        // drains all pending notifications, and writes the current
        // state exactly once for the whole burst.
        tokio::time::advance(SAVE_DEBOUNCE * 3).await;
        // Let the worker run its save and park on `notify.notified()`.
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;

        let writes = snapshot_store.write_count();
        assert!(
            writes <= 3,
            "expected ≤3 file writes after 200-commit burst, got {writes}"
        );
        assert!(writes >= 1, "expected at least one save, got {writes}");
    }

    /// Focused debug test for the known_peers+auto-resync feature
    /// path. Seeds A and B, does share→accept, then:
    ///   1. verifies A has B's addr in its sidecar after the accept
    ///   2. verifies that after a direct `sync_with_peers` from A, B's sidecar
    ///      records A's addr too
    ///   3. verifies `remember_peer` replaces an entry when a newer addr for
    ///      the same EndpointId arrives
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn known_peers_sidecar_round_trip() {
        let (backend_a, _dir_a) = make_backend();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "Shared").await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let resp = backend_a
            .share_subtree("block:shared", "none".into())
            .await
            .unwrap();
        let j: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            _ => panic!(),
        };
        let ticket = j["ticket"].as_str().unwrap().to_string();
        let shared_tree_id = j["shared_tree_id"].as_str().unwrap().to_string();

        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();

        // Give the advertiser callback on A time to fire and persist.
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // After accept, A's sidecar should hold B's addr (the dialer
        // side of B's initial `sync_doc_initiate` came through A's
        // accept_loop callback).
        let a_peers = backend_a
            .snapshot_store()
            .load_peers(&shared_tree_id)
            .unwrap();
        assert_eq!(
            a_peers.len(),
            1,
            "A should know exactly B after accept, got: {a_peers:?}"
        );

        // B's sidecar should have A's addr (recorded on accept).
        let b_peers = backend_b
            .snapshot_store()
            .load_peers(&shared_tree_id)
            .unwrap();
        assert_eq!(
            b_peers.len(),
            1,
            "B should know exactly A after accept, got: {b_peers:?}"
        );

        backend_a.advertiser_for_test().close_all().await;
        backend_b.advertiser_for_test().close_all().await;
    }

    /// Full-stack test of the known_peers sidecar + stable iroh
    /// endpoint identity + auto-resync path. After A restarts, a
    /// manual sync from B to A must succeed (addr resolved from
    /// sidecar, endpoint key still valid). This is the minimum
    /// reproducer — the PBT exercises the same flow with random
    /// interleavings and the full auto-resync timing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn cross_peer_sync_after_restart_debug() {
        let (backend_a, dir_a) = make_backend();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "Shared").await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let resp = backend_a
            .share_subtree("block:shared", "none".into())
            .await
            .unwrap();
        let j: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            _ => panic!(),
        };
        let ticket = j["ticket"].as_str().unwrap().to_string();
        let shared_tree_id = j["shared_tree_id"].as_str().unwrap().to_string();

        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();

        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // Pre-restart: A must know B (from the accept-loop callback
        // that fired when B dialled in during `accept_shared_subtree`).
        let a_peers_pre = backend_a
            .snapshot_store()
            .load_peers(&shared_tree_id)
            .unwrap();
        assert_eq!(a_peers_pre.len(), 1);

        // Restart A. Drop advertiser + backend, spin up fresh, rehydrate.
        backend_a.advertiser_for_test().close_all().await;
        backend_a.flush_all().await;
        let dir_a_path = dir_a.path().to_path_buf();
        drop(backend_a);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let bus = Arc::new(DegradedSignalBus::new());
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(dir_a_path.clone())));
        let snapshot_store = Arc::new(SharedSnapshotStore::new(dir_a_path.clone(), bus.clone()));
        let manager = Arc::new(SharedTreeSyncManager::new());
        let key = crate::device_key_store::load_or_create_device_key(&dir_a_path).unwrap();
        let advertiser = Arc::new(IrohAdvertiser::new_with_key(key.clone()));
        let backend_a = LoroShareBackend::new(store, snapshot_store, manager, advertiser, bus, key);
        let collab = backend_a.test_global_doc().await;
        let doc_arc = collab.doc();
        let doc = &*doc_arc;
        let n = rehydrate_shared_trees(&backend_a, doc).await.unwrap();
        assert_eq!(n, 1, "A should rehydrate exactly 1 share");

        // Let the rehydrate kick-sync dial B so B records A's fresh
        // addr via the accept-loop callback.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        // Edit on B. B's auto-resync worker will debounce 500 ms then
        // dial A using the addr B just refreshed above.
        {
            let d = backend_b
                .manager_for_test()
                .get_doc(&shared_tree_id)
                .unwrap();
            let tree = d.get_tree(crate::loro_backend::TREE_NAME);
            let root = tree.roots()[0];
            let meta = tree.get_meta(root).unwrap();
            let text = match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t,
                _ => panic!("no content_raw on B's shared root"),
            };
            let len = text.len_unicode();
            text.insert(len, " [edit-from-B]").unwrap();
            d.commit();
        }

        // Wait for B's auto-resync to fire and A to import B's delta.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            let a_doc = backend_a
                .manager_for_test()
                .get_doc(&shared_tree_id)
                .expect("A has shared doc");
            let a_tree = a_doc.get_tree(crate::loro_backend::TREE_NAME);
            let root = a_tree.roots()[0];
            let meta = a_tree.get_meta(root).unwrap();
            let text = match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t.to_string(),
                _ => String::new(),
            };
            if text.contains("[edit-from-B]") {
                tracing::debug!("[debug] A picked up B's edit: {text}");
                break;
            }
            if std::time::Instant::now() >= deadline {
                panic!("A did not pick up B's edit within 8s: {text}");
            }
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        backend_a.advertiser_for_test().close_all().await;
        backend_b.advertiser_for_test().close_all().await;
    }

    /// Regression for the peer-id counter-reuse divergence (B2): a
    /// restart from a snapshot older than an already-pushed edit must
    /// mint a FRESH peer-id (generation bump) so subsequent edits cannot
    /// reuse an already-used `(peer_id, counter)`, and a bidirectional
    /// sync must still converge A and B to identical shared-tree text.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn convergence_after_stale_snapshot_restart() {
        fn shared_root_text(backend: &LoroShareBackend, shared_tree_id: &str) -> String {
            let d = backend
                .manager_for_test()
                .get_doc(shared_tree_id)
                .expect("shared doc registered");
            let tree = d.get_tree(crate::loro_backend::TREE_NAME);
            let root = tree.roots()[0];
            let meta = tree.get_meta(root).unwrap();
            match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t.to_string(),
                _ => String::new(),
            }
        }
        fn edit_shared_root(backend: &LoroShareBackend, shared_tree_id: &str, suffix: &str) {
            let d = backend
                .manager_for_test()
                .get_doc(shared_tree_id)
                .expect("shared doc registered");
            let tree = d.get_tree(crate::loro_backend::TREE_NAME);
            let root = tree.roots()[0];
            let meta = tree.get_meta(root).unwrap();
            let text = match meta.get("content_raw") {
                Some(loro::ValueOrContainer::Container(loro::Container::Text(t))) => t,
                _ => panic!("no content_raw on shared root"),
            };
            let len = text.len_unicode();
            text.insert(len, suffix).unwrap();
            d.commit();
        }

        let (backend_a, dir_a) = make_backend();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "Shared").await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let resp = backend_a
            .share_subtree("block:shared", "none".into())
            .await
            .unwrap();
        let j: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            _ => panic!(),
        };
        let ticket = j["ticket"].as_str().unwrap().to_string();
        let shared_tree_id = j["shared_tree_id"].as_str().unwrap().to_string();

        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;

        // A pushes an edit to B — the ops are now on B, but A's debounced
        // snapshot may lag (this is exactly the crash window).
        edit_shared_root(&backend_a, &shared_tree_id, " [edit-A-pre-restart]");
        backend_a.sync_with_peers(&shared_tree_id).await.unwrap();

        // Wait for B to import A's pushed edit.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            if shared_root_text(&backend_b, &shared_tree_id).contains("[edit-A-pre-restart]") {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "B never imported A's pre-restart edit"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        let a_peer_pre = backend_a
            .manager_for_test()
            .get_doc(&shared_tree_id)
            .unwrap()
            .peer_id();

        // Restart A from disk (same device key → same key material, so
        // any divergence would come purely from peer-id reuse).
        backend_a.advertiser_for_test().close_all().await;
        backend_a.flush_all().await;
        let dir_a_path = dir_a.path().to_path_buf();
        drop(backend_a);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;

        let bus = Arc::new(DegradedSignalBus::new());
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(dir_a_path.clone())));
        let snapshot_store = Arc::new(SharedSnapshotStore::new(dir_a_path.clone(), bus.clone()));
        let manager = Arc::new(SharedTreeSyncManager::new());
        let key = crate::device_key_store::load_or_create_device_key(&dir_a_path).unwrap();
        let advertiser = Arc::new(IrohAdvertiser::new_with_key(key.clone()));
        let backend_a = LoroShareBackend::new(store, snapshot_store, manager, advertiser, bus, key);
        let collab = backend_a.test_global_doc().await;
        let doc_arc = collab.doc();
        let n = rehydrate_shared_trees(&backend_a, &doc_arc).await.unwrap();
        assert_eq!(n, 1, "A should rehydrate exactly 1 share");

        // The generation bump must have handed A a FRESH peer-id — this is
        // what makes counter reuse structurally impossible.
        let a_peer_post = backend_a
            .manager_for_test()
            .get_doc(&shared_tree_id)
            .unwrap()
            .peer_id();
        assert_ne!(
            a_peer_pre, a_peer_post,
            "rehydrate must mint a fresh peer-id via the generation bump"
        );

        // Let A's rehydrate kick-sync refresh B's addr for A.
        tokio::time::sleep(std::time::Duration::from_millis(1500)).await;

        // A makes a NEW edit under its fresh peer-id and pushes it.
        edit_shared_root(&backend_a, &shared_tree_id, " [edit-A-post-restart]");
        backend_a.sync_with_peers(&shared_tree_id).await.unwrap();

        // Both peers must converge to identical text containing both edits.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        loop {
            let ta = shared_root_text(&backend_a, &shared_tree_id);
            let tb = shared_root_text(&backend_b, &shared_tree_id);
            if ta == tb
                && ta.contains("[edit-A-pre-restart]")
                && ta.contains("[edit-A-post-restart]")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "A and B did not converge after stale-snapshot restart:\n  A: {ta}\n  B: {tb}"
            );
            tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        }

        backend_a.advertiser_for_test().close_all().await;
        backend_b.advertiser_for_test().close_all().await;
    }

    /// chmod the `shares/` directory to read-only, commit an edit,
    /// and assert the worker emits `ShareDegraded::SnapshotSaveFailed`
    /// while keeping the in-memory doc's edit intact.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn readonly_shares_dir_emits_degraded() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let bus = Arc::new(DegradedSignalBus::new());
        let snapshot_store = Arc::new(SharedSnapshotStore::new(
            dir.path().to_path_buf(),
            bus.clone(),
        ));

        // Materialise `shares/` first (save once so the dir exists and
        // has at least one successful write), then chmod it read-only.
        let doc = Arc::new(LoroDoc::new());
        snapshot_store.save("readonly", &doc).unwrap();

        let shares_dir = dir.path().join("shares");
        let orig_perm = std::fs::metadata(&shares_dir).unwrap().permissions();
        std::fs::set_permissions(&shares_dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        let mut rx = bus.subscribe().changes;
        let _worker = spawn_save_worker(
            snapshot_store.clone(),
            bus.clone(),
            "readonly".to_string(),
            doc.clone(),
        );

        // Commit an edit that the worker will try to persist.
        {
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            let node = tree.create(None::<TreeID>).unwrap();
            tree.get_meta(node).unwrap().insert("k", "v").unwrap();
            doc.commit();
        }

        // Wait up to 1s for the degraded signal — debounce is 150ms
        // so the save attempt should fire within ~200ms.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(1);
        let ev = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(change)) => change.raised().expect("expected Raised"),
            Ok(Err(_)) => panic!("bus closed unexpectedly"),
            Err(_) => panic!("no ShareDegraded event within 1s"),
        };
        assert_eq!(ev.shared_tree_id, "readonly");
        assert!(matches!(
            ev.reason,
            ShareDegradedReason::SnapshotSaveFailed(_)
        ));

        // In-memory doc still has the edit — failure must not
        // roll back the state the user produced.
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let nodes: Vec<_> = tree
            .get_nodes(false)
            .into_iter()
            .filter(|n| !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist))
            .collect();
        assert!(!nodes.is_empty(), "in-memory edit should be intact");

        // Restore perms so TempDir can clean up.
        std::fs::set_permissions(&shares_dir, orig_perm).unwrap();
    }

    // ---- Phase 5 lifecycle fixes: SQL projection + unshare ----

    /// In-memory `OriginTaggedWrites` that records block rows keyed by `id`,
    /// with `INSERT OR IGNORE` create semantics (mirrors the SQL `block` table
    /// the projection targets). Lets backend-only tests assert what the sharer
    /// projected without the full DI/SQL stack.
    #[derive(Default)]
    struct RecordingSqlOps {
        rows: std::sync::Mutex<HashMap<String, StorageEntity>>,
    }

    impl RecordingSqlOps {
        fn get(&self, id: &str) -> Option<StorageEntity> {
            self.rows.lock().unwrap().get(id).cloned()
        }

        fn apply(&self, op: &str, params: StorageEntity) {
            let id = params
                .get("id")
                .and_then(|v| v.as_string())
                .expect("op params must carry an `id`")
                .to_string();
            let mut rows = self.rows.lock().unwrap();
            match op {
                // INSERT OR IGNORE — first writer of an id wins.
                "create" => {
                    rows.entry(id).or_insert(params);
                }
                "delete" => {
                    rows.remove(&id);
                }
                // update / set_field — upsert.
                _ => {
                    rows.insert(id, params);
                }
            }
        }
    }

    #[async_trait]
    impl OperationProvider for RecordingSqlOps {
        fn operations(&self) -> Vec<OperationDescriptor> {
            vec![]
        }
        async fn execute_operation(
            &self,
            _: &EntityName,
            op_name: &str,
            params: StorageEntity,
        ) -> Result<OperationResult> {
            self.apply(op_name, params);
            Ok(OperationResult::irreversible(vec![]))
        }
    }

    #[async_trait]
    impl OriginTaggedWrites for RecordingSqlOps {
        async fn execute_operation_with_origin(
            &self,
            _: &EntityName,
            op_name: &str,
            params: StorageEntity,
            _: crate::event_bus::EventOrigin,
        ) -> Result<OperationResult> {
            self.apply(op_name, params);
            Ok(OperationResult::irreversible(vec![]))
        }
        async fn execute_batch_with_origin(
            &self,
            _: &EntityName,
            operations: Vec<(String, StorageEntity)>,
            _: crate::event_bus::EventOrigin,
        ) -> Result<Vec<OperationResult>> {
            let mut out = Vec::with_capacity(operations.len());
            for (op, params) in operations {
                self.apply(&op, params);
                out.push(OperationResult::irreversible(vec![]));
            }
            Ok(out)
        }
    }

    /// `OriginTaggedWrites` whose batch path always fails — drives the
    /// projection worker's degraded-signal path (Fix 2).
    struct FailingSqlOps;

    #[async_trait]
    impl OperationProvider for FailingSqlOps {
        fn operations(&self) -> Vec<OperationDescriptor> {
            vec![]
        }
        async fn execute_operation(
            &self,
            _: &EntityName,
            _: &str,
            _: StorageEntity,
        ) -> Result<OperationResult> {
            Err(err("stub sql: execute_operation always fails"))
        }
    }

    #[async_trait]
    impl OriginTaggedWrites for FailingSqlOps {
        async fn execute_operation_with_origin(
            &self,
            _: &EntityName,
            _: &str,
            _: StorageEntity,
            _: crate::event_bus::EventOrigin,
        ) -> Result<OperationResult> {
            Err(err("stub sql: execute_operation_with_origin always fails"))
        }
        async fn execute_batch_with_origin(
            &self,
            _: &EntityName,
            _: Vec<(String, StorageEntity)>,
            _: crate::event_bus::EventOrigin,
        ) -> Result<Vec<OperationResult>> {
            Err(err("stub sql: batch write failure"))
        }
    }

    /// Like `make_backend`, but wires a `RecordingSqlOps` so mount/descendant
    /// projection writes are observable. `downstream_projection` is `None` —
    /// there is no global `LoroSyncController` loop in these tests, so no
    /// prune-delete races the projection and the flush barrier is unnecessary.
    fn make_backend_with_sql() -> (Arc<LoroShareBackend>, Arc<RecordingSqlOps>, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(
            dir.path().to_path_buf(),
        )));
        let bus = Arc::new(DegradedSignalBus::new());
        let snapshot_store = Arc::new(SharedSnapshotStore::new(
            dir.path().to_path_buf(),
            bus.clone(),
        ));
        let manager = Arc::new(SharedTreeSyncManager::new());
        let key = crate::device_key_store::load_or_create_device_key(dir.path()).unwrap();
        let advertiser = Arc::new(IrohAdvertiser::new_with_key(key.clone()));
        let sql = Arc::new(RecordingSqlOps::default());
        let backend = LoroShareBackend::new_with_sql(
            store,
            snapshot_store,
            manager,
            advertiser,
            bus,
            key,
            Some(sql.clone() as Arc<dyn OriginTaggedWrites>),
            None,
        );
        (backend, sql, dir)
    }

    /// Fix 2: a projection worker whose `sql_ops` batch fails must emit a
    /// `ShareDegraded { SqlProjectionFailed }` (not just log), so the UI can
    /// surface the Loro↔SQL divergence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn projection_worker_failure_emits_degraded() {
        let bus = Arc::new(DegradedSignalBus::new());
        let failing: Arc<dyn OriginTaggedWrites> = Arc::new(FailingSqlOps);
        let doc = Arc::new(LoroDoc::new());
        let mut rx = bus.subscribe().changes;

        // Empty global doc — `proj-child` is not a local block, so the
        // collision guard passes and the op reaches the failing sink.
        let global =
            Arc::new(crate::loro_document::LoroDocument::new("proj-global".to_string()).unwrap());

        // Spawn BEFORE the commit so the watermark starts empty and the commit
        // produces a create op the failing sink rejects.
        let _worker = spawn_projection_worker(
            doc.clone(),
            failing,
            bus.clone(),
            "block:mount".to_string(),
            "proj-share".to_string(),
            global,
        );

        {
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            let node = tree.create(None::<TreeID>).unwrap();
            let meta = tree.get_meta(node).unwrap();
            meta.insert(STABLE_ID, loro::LoroValue::from("proj-child"))
                .unwrap();
            let text: loro::LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
            text.insert(0, "child content").unwrap();
            doc.commit();
        }

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let ev = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(change)) => change.raised().expect("expected Raised"),
            Ok(Err(_)) => panic!("bus closed unexpectedly"),
            Err(_) => panic!("no ShareDegraded event within 2s"),
        };
        assert_eq!(ev.shared_tree_id, "proj-share");
        assert!(
            matches!(ev.reason, ShareDegradedReason::SqlProjectionFailed(_)),
            "expected SqlProjectionFailed, got {:?}",
            ev.reason
        );
    }

    /// Integrity (B-integrity): a synced-in shared-doc edit whose block id
    /// collides with a LIVE local block id (e.g. a hostile sharer naming a node
    /// `journals` to shadow the recipient's `block:journals`) must be REFUSED —
    /// the projection worker emits `ForeignIdCollision` and never writes the
    /// clobbering op, so the recipient's own SQL row survives intact.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn projection_worker_refuses_local_id_collision() {
        use crate::loro_backend::TREE_NAME;

        let bus = Arc::new(DegradedSignalBus::new());
        let sql = Arc::new(RecordingSqlOps::default());
        let mut rx = bus.subscribe().changes;

        // The recipient already owns a local `block:journals` row (the UI reads
        // this). A hostile share must not be able to overwrite it.
        {
            let mut local = StorageEntity::new();
            local.insert("id".into(), Value::String("block:journals".into()));
            local.insert("content".into(), Value::String("MY PRIVATE JOURNAL".into()));
            sql.apply("create", local);
        }

        // Global tree holds a LIVE node with stable id `journals` — the
        // authority that says "this id is a local block".
        let global = Arc::new(
            crate::loro_document::LoroDocument::new("collide-global".to_string()).unwrap(),
        );
        {
            let gdoc = global.doc();
            let tree = gdoc.get_tree(TREE_NAME);
            let node = tree.create(None::<TreeID>).unwrap();
            let meta = tree.get_meta(node).unwrap();
            meta.insert(STABLE_ID, loro::LoroValue::from("journals"))
                .unwrap();
            gdoc.commit();
        }

        // Malicious shared doc: a node reusing the well-known id `journals`.
        let doc = Arc::new(LoroDoc::new());
        let _worker = spawn_projection_worker(
            doc.clone(),
            sql.clone() as Arc<dyn OriginTaggedWrites>,
            bus.clone(),
            "block:mount".to_string(),
            "hostile-share".to_string(),
            global,
        );

        {
            let tree = doc.get_tree(TREE_NAME);
            let node = tree.create(None::<TreeID>).unwrap();
            let meta = tree.get_meta(node).unwrap();
            meta.insert(STABLE_ID, loro::LoroValue::from("journals"))
                .unwrap();
            let text: loro::LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
            text.insert(0, "PWNED").unwrap();
            doc.commit();
        }

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        let ev = match tokio::time::timeout_at(deadline, rx.recv()).await {
            Ok(Ok(change)) => change.raised().expect("expected Raised"),
            Ok(Err(_)) => panic!("bus closed unexpectedly"),
            Err(_) => panic!("no ShareDegraded event within 2s"),
        };
        assert_eq!(ev.shared_tree_id, "hostile-share");
        assert!(
            matches!(
                &ev.reason,
                ShareDegradedReason::ForeignIdCollision(id) if id == "block:journals"
            ),
            "expected ForeignIdCollision(block:journals), got {:?}",
            ev.reason
        );

        // The recipient's local row is untouched — no clobbering op ran.
        let row = sql
            .get("block:journals")
            .expect("local journal row must survive");
        assert_eq!(
            row.get("content").and_then(|v| v.as_string()),
            Some("MY PRIVATE JOURNAL"),
            "hostile share must NOT overwrite the recipient's local journal"
        );
    }

    /// Fix 1: `share_subtree` must project the mount + descendants into SQL so
    /// the sharing peer's UI keeps rendering the subtree (it reads SQL, not
    /// Loro). Without this the sharer loses the subtree until restart.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn share_subtree_projects_descendants_into_sql() {
        let (backend, sql, _dir) = make_backend_with_sql();
        seed_block(&backend, "root-a", None, "root-a").await;
        seed_block(&backend, "shared-parent", Some("root-a"), "Shared heading").await;
        seed_block(
            &backend,
            "shared-child",
            Some("shared-parent"),
            "Shared child",
        )
        .await;

        let resp = backend
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();
        let json: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response: {other:?}"),
        };
        let mount_id = json["mount_block_id"].as_str().unwrap().to_string();

        // Mount row projected as a PAGE (Inc 2) so the shared subtree owns a
        // dedicated org file. `shared-parent` is a plain BLOCK, so this is the
        // SYNTHETIC-CONTAINER case (D3): the mount is a synthetic page wrapping
        // it. Amendment A: `root-a` is a non-page ROOT, so the mount-page cannot
        // sit under it (no pages under non-pages) and has no page ancestor —
        // it bubbles to the top (`no_parent`), NOT under `root-a`.
        let mount = sql
            .get(&mount_id)
            .expect("mount row must be projected into SQL");
        assert_eq!(
            mount.get("parent_id").and_then(|v| v.as_string()),
            Some(EntityUri::no_parent().as_str()),
            "mount-page bubbles above the non-page root-a (Amendment A)"
        );
        assert_eq!(
            mount.get(SHARE_ROLE_PROPERTY).and_then(|v| v.as_string()),
            Some(SHARE_ROLE_MOUNT)
        );
        assert!(
            matches!(mount.get("tags"), Some(Value::Array(tags))
                if tags.iter().any(|t| t.as_string() == Some(holon_api::block::PAGE_TAG))),
            "mount must be tagged Page so it owns a file; got {:?}",
            mount.get("tags")
        );

        // Shared root (shared-parent) re-parented under the mount (synthetic
        // container keeps the shared block's own node).
        let parent_row = sql
            .get("block:shared-parent")
            .expect("shared-parent row must be projected");
        assert_eq!(
            parent_row.get("parent_id").and_then(|v| v.as_string()),
            Some(mount_id.as_str())
        );

        // Shared child projected, still under its shared parent.
        let child_row = sql
            .get("block:shared-child")
            .expect("shared-child row must be projected");
        assert_eq!(
            child_row.get("parent_id").and_then(|v| v.as_string()),
            Some("block:shared-parent")
        );

        backend.advertiser.close_all().await;
    }

    /// D3 adopt-and-collapse (page share): sharing a PAGE P makes the mount
    /// page ADOPT P's identity — the mount carries P's title, P's OWN row
    /// is folded out of the projection, and P's children reparent to the
    /// mount. P stays uncollapsed in the shared Loro doc (CRDT truth); only
    /// the SQL/org projection folds it. Because P's parent (`root-page`) is
    /// already a page, the mount stays in place under it (no Amendment-A
    /// bubbling).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn share_page_adopts_identity_and_folds_root() {
        let (backend, sql, _dir) = make_backend_with_sql();
        seed_page(&backend, "root-page", None, "Root Page").await;
        seed_page(&backend, "shared-page", Some("root-page"), "My Shared Page").await;
        seed_block(&backend, "p-child", Some("shared-page"), "Child under P").await;

        let resp = backend
            .share_subtree("block:shared-page", "none".into())
            .await
            .unwrap();
        let json: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response: {other:?}"),
        };
        let mount_id = json["mount_block_id"].as_str().unwrap().to_string();

        // Mount adopts P's title, is a Page, and stays under the page `root-page`.
        let mount = sql.get(&mount_id).expect("mount row must be projected");
        assert_eq!(
            mount.get("content").and_then(|v| v.as_string()),
            Some("My Shared Page"),
            "mount adopts the shared page's title"
        );
        assert_eq!(
            mount.get("parent_id").and_then(|v| v.as_string()),
            Some("block:root-page"),
            "mount stays in place under the page root-page"
        );
        assert!(
            matches!(mount.get("tags"), Some(Value::Array(tags))
                if tags.iter().any(|t| t.as_string() == Some(holon_api::block::PAGE_TAG))),
            "mount is a Page"
        );

        // P's OWN row is folded out — the mount IS P now.
        assert!(
            sql.get("block:shared-page").is_none(),
            "shared page P's own row must be folded onto the mount, not projected"
        );

        // P's child reparents directly to the mount.
        let child = sql
            .get("block:p-child")
            .expect("P's child must be projected");
        assert_eq!(
            child.get("parent_id").and_then(|v| v.as_string()),
            Some(mount_id.as_str()),
            "P's children reparent to the mount"
        );

        backend.advertiser.close_all().await;
    }

    /// D3 acceptor side — PAGE share (adopt-and-collapse). Accepting a shared
    /// PAGE projects the mount on the ACCEPTOR as a Page that adopts P's title,
    /// folds P's own row, and reparents P's children to the mount — symmetric
    /// with the sharer (Amendment B), driven through the real iroh round-trip.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn accept_page_adopts_identity_and_folds_root() {
        let (backend_a, _dir_a) = make_backend();
        let (backend_b, sql_b, _dir_b) = make_backend_with_sql();

        seed_page(&backend_a, "root-a", None, "Root A").await;
        seed_page(&backend_a, "shared-page", Some("root-a"), "My Shared Page").await;
        seed_block(&backend_a, "p-child", Some("shared-page"), "Child under P").await;
        // The acceptor needs a page to mount under (kept in place).
        seed_page(&backend_b, "root-b", None, "Root B").await;

        let ticket = {
            let resp = backend_a
                .share_subtree("block:shared-page", "none".into())
                .await
                .unwrap();
            let tj: serde_json::Value = match resp.response.unwrap() {
                Value::String(s) => serde_json::from_str(&s).unwrap(),
                o => panic!("unexpected: {o:?}"),
            };
            tj["ticket"].as_str().unwrap().to_string()
        };

        let accept_resp = backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();
        let mount_id = match accept_resp.response.unwrap() {
            Value::String(s) => {
                serde_json::from_str::<serde_json::Value>(&s).unwrap()["mount_block_id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            }
            o => panic!("unexpected: {o:?}"),
        };

        let mount = sql_b.get(&mount_id).expect("mount row on acceptor");
        assert_eq!(
            mount.get("content").and_then(|v| v.as_string()),
            Some("My Shared Page"),
            "acceptor mount adopts P's title"
        );
        assert_eq!(
            mount.get("parent_id").and_then(|v| v.as_string()),
            Some("block:root-b"),
            "acceptor mount sits under the chosen page target"
        );
        assert!(
            matches!(mount.get("tags"), Some(Value::Array(t))
                if t.iter().any(|x| x.as_string() == Some(holon_api::block::PAGE_TAG))),
            "acceptor mount is a Page"
        );
        assert!(
            sql_b.get("block:shared-page").is_none(),
            "P folds on the acceptor too"
        );
        let child = sql_b.get("block:p-child").expect("P's child on acceptor");
        assert_eq!(
            child.get("parent_id").and_then(|v| v.as_string()),
            Some(mount_id.as_str()),
            "P's children reparent to the acceptor mount"
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// D3 acceptor side — BLOCK share (synthetic container). Accepting a shared
    /// plain BLOCK projects the mount on the ACCEPTOR as a synthetic Page that
    /// wraps the shared block (the block keeps its own node under the mount).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn accept_block_is_synthetic_container() {
        let (backend_a, _dir_a) = make_backend();
        let (backend_b, sql_b, _dir_b) = make_backend_with_sql();

        // `shared-block` lives under a page on A so sharing it is legal.
        seed_page(&backend_a, "root-a", None, "Root A").await;
        seed_block(
            &backend_a,
            "shared-block",
            Some("root-a"),
            "Shared block heading",
        )
        .await;
        seed_block(
            &backend_a,
            "sb-child",
            Some("shared-block"),
            "child of the block",
        )
        .await;
        seed_page(&backend_b, "root-b", None, "Root B").await;

        let ticket = {
            let resp = backend_a
                .share_subtree("block:shared-block", "none".into())
                .await
                .unwrap();
            let tj: serde_json::Value = match resp.response.unwrap() {
                Value::String(s) => serde_json::from_str(&s).unwrap(),
                o => panic!("unexpected: {o:?}"),
            };
            tj["ticket"].as_str().unwrap().to_string()
        };

        let accept_resp = backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();
        let mount_id = match accept_resp.response.unwrap() {
            Value::String(s) => {
                serde_json::from_str::<serde_json::Value>(&s).unwrap()["mount_block_id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            }
            o => panic!("unexpected: {o:?}"),
        };

        let mount = sql_b.get(&mount_id).expect("mount row on acceptor");
        assert!(
            mount
                .get("content")
                .and_then(|v| v.as_string())
                .is_some_and(|c| c.contains("Shared tree")),
            "synthetic container uses the synthetic title, got {:?}",
            mount.get("content")
        );
        assert!(
            matches!(mount.get("tags"), Some(Value::Array(t))
                if t.iter().any(|x| x.as_string() == Some(holon_api::block::PAGE_TAG))),
            "acceptor synthetic mount is a Page"
        );
        // The shared block keeps its own node, reparented under the mount.
        let sb = sql_b
            .get("block:shared-block")
            .expect("shared block row projected (not folded)");
        assert_eq!(
            sb.get("parent_id").and_then(|v| v.as_string()),
            Some(mount_id.as_str()),
            "shared block reparents under the synthetic mount"
        );
        let child = sql_b.get("block:sb-child").expect("shared block's child");
        assert_eq!(
            child.get("parent_id").and_then(|v| v.as_string()),
            Some("block:shared-block"),
            "the block's own subtree is preserved under it"
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// N3 / ADR 0028 H7 — recipient-side orphan fix. Accepting a share under a
    /// target that has NO page ancestor must NOT orphan the mount at
    /// `no_parent` (present in SQL but invisible in the UI — dogfood
    /// 2026-07-20). The mount must land under the dedicated **"Shared with
    /// me"** recipient root, and that root must itself be projected as a
    /// rendered top-level page so the accepted content is reachable.
    ///
    /// RED (pre-fix): the mount row's `parent_id` is `sentinel:no_parent` and
    /// no `block:shared-with-me` row exists → both assertions below fail.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn accept_orphan_target_lands_under_shared_with_me_root() {
        let (backend_a, _dir_a) = make_backend();
        let (backend_b, sql_b, _dir_b) = make_backend_with_sql();

        // A shares a plain block that lives under a page (legal to share).
        seed_page(&backend_a, "root-a", None, "Root A").await;
        seed_block(&backend_a, "shared-block", Some("root-a"), "Shared heading").await;

        // B's accept target is a plain, NON-page top-level block: it has no page
        // ancestor, so the mount would otherwise bubble to `no_parent`.
        seed_block(&backend_b, "host-b", None, "Host B").await;

        let ticket = {
            let resp = backend_a
                .share_subtree("block:shared-block", "none".into())
                .await
                .unwrap();
            let tj: serde_json::Value = match resp.response.unwrap() {
                Value::String(s) => serde_json::from_str(&s).unwrap(),
                o => panic!("unexpected: {o:?}"),
            };
            tj["ticket"].as_str().unwrap().to_string()
        };

        let accept_resp = backend_b
            .accept_shared_subtree("block:host-b", ticket)
            .await
            .unwrap();
        let mount_id = match accept_resp.response.unwrap() {
            Value::String(s) => {
                serde_json::from_str::<serde_json::Value>(&s).unwrap()["mount_block_id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            }
            o => panic!("unexpected: {o:?}"),
        };

        let shared_with_me_uri = block_uri_from_bare(SHARED_WITH_ME_ROOT_ID);

        // (1) The mount attaches under the "Shared with me" root, NOT no_parent.
        let mount = sql_b.get(&mount_id).expect("mount row on acceptor");
        assert_eq!(
            mount.get("parent_id").and_then(|v| v.as_string()),
            Some(shared_with_me_uri.as_str()),
            "orphan-target mount must attach under the 'Shared with me' root \
             (H7), not orphan at no_parent; got {:?}",
            mount.get("parent_id")
        );

        // (2) The "Shared with me" root is a rendered top-level page.
        let root = sql_b
            .get(&shared_with_me_uri)
            .expect("'Shared with me' root row must be projected so the UI renders it");
        assert_eq!(
            root.get("parent_id").and_then(|v| v.as_string()),
            Some(EntityUri::no_parent().as_str()),
            "'Shared with me' root is a top-level page"
        );
        assert_eq!(
            root.get("content").and_then(|v| v.as_string()),
            Some(SHARED_WITH_ME_TITLE),
        );
        assert!(
            matches!(root.get("tags"), Some(Value::Array(t))
                if t.iter().any(|x| x.as_string() == Some(holon_api::block::PAGE_TAG))),
            "'Shared with me' root must be tagged Page so it renders as a page"
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// H7 idempotency: TWO orphan-target accepts on the SAME device must reuse
    /// ONE "Shared with me" root, not mint a second one. Guards the
    /// `ensure_shared_with_me_root_node` short-circuit + the root row's
    /// INSERT-OR-IGNORE projection. Both distinct shares' mounts land under the
    /// single shared root.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn two_orphan_accepts_reuse_one_shared_with_me_root() {
        let (backend_a, _dir_a) = make_backend();
        let (backend_b, sql_b, _dir_b) = make_backend_with_sql();

        // A shares two independent blocks (each under a page, so sharing is legal).
        seed_page(&backend_a, "root-a", None, "Root A").await;
        seed_block(&backend_a, "share-one", Some("root-a"), "Share one").await;
        seed_block(&backend_a, "share-two", Some("root-a"), "Share two").await;

        // B's accept target is a plain, NON-page top-level block (no page
        // ancestor) so BOTH accepts hit the orphan → "Shared with me" path.
        seed_block(&backend_b, "host-b", None, "Host B").await;

        let ticket_for = |backend: &Arc<LoroShareBackend>, id: &str| {
            let backend = backend.clone();
            let id = id.to_string();
            async move {
                let resp = backend.share_subtree(&id, "none".into()).await.unwrap();
                let tj: serde_json::Value = match resp.response.unwrap() {
                    Value::String(s) => serde_json::from_str(&s).unwrap(),
                    o => panic!("unexpected: {o:?}"),
                };
                tj["ticket"].as_str().unwrap().to_string()
            }
        };
        let ticket_one = ticket_for(&backend_a, "block:share-one").await;
        let ticket_two = ticket_for(&backend_a, "block:share-two").await;

        let accept_mount = |backend: &Arc<LoroShareBackend>, ticket: String| {
            let backend = backend.clone();
            async move {
                let resp = backend
                    .accept_shared_subtree("block:host-b", ticket)
                    .await
                    .unwrap();
                match resp.response.unwrap() {
                    Value::String(s) => {
                        serde_json::from_str::<serde_json::Value>(&s).unwrap()["mount_block_id"]
                            .as_str()
                            .unwrap()
                            .to_string()
                    }
                    o => panic!("unexpected: {o:?}"),
                }
            }
        };
        let mount_one = accept_mount(&backend_b, ticket_one).await;
        // Second accept (distinct share) must succeed, not fail on the
        // already-existing root.
        let mount_two = accept_mount(&backend_b, ticket_two).await;
        assert_ne!(
            mount_one, mount_two,
            "two distinct shares get distinct mounts"
        );

        let shared_with_me_uri = block_uri_from_bare(SHARED_WITH_ME_ROOT_ID);

        // Exactly ONE "Shared with me" node in B's global Loro tree.
        let loro_root_count = {
            let collab = backend_b.global_doc().await.unwrap();
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            tree.get_nodes(false)
                .iter()
                .filter(|n| !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist))
                .filter(|n| read_stable_id(&tree, n.id).as_deref() == Some(SHARED_WITH_ME_ROOT_ID))
                .count()
        };
        assert_eq!(
            loro_root_count, 1,
            "exactly one 'Shared with me' root node must exist in the Loro tree, found {loro_root_count}"
        );

        // Exactly ONE "Shared with me" row in SQL (keyed by id; assert present).
        let root = sql_b
            .get(&shared_with_me_uri)
            .expect("'Shared with me' root row must be projected");
        assert_eq!(
            root.get("content").and_then(|v| v.as_string()),
            Some(SHARED_WITH_ME_TITLE),
        );

        // Both mounts sit under the single shared root.
        for mount_id in [&mount_one, &mount_two] {
            let mount = sql_b.get(mount_id).expect("mount row on acceptor");
            assert_eq!(
                mount.get("parent_id").and_then(|v| v.as_string()),
                Some(shared_with_me_uri.as_str()),
                "mount {mount_id} must attach under the single 'Shared with me' root"
            );
        }

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// Fix 3: `unshare` stops advertising, unregisters the doc, removes the
    /// mount node + SQL rows, deletes the snapshot, and — because the workers
    /// are dropped first — a later commit to the detached doc does NOT
    /// resurrect the snapshot (the gc-resurrection guard).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn unshare_tears_down_share_and_prevents_resurrection() {
        let (backend, sql, _dir) = make_backend_with_sql();
        seed_block(&backend, "root-a", None, "root-a").await;
        seed_block(&backend, "shared-parent", Some("root-a"), "Shared heading").await;
        seed_block(
            &backend,
            "shared-child",
            Some("shared-parent"),
            "Shared child",
        )
        .await;

        let resp = backend
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();
        let json: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response: {other:?}"),
        };
        let mount_id = json["mount_block_id"].as_str().unwrap().to_string();
        let stid = json["shared_tree_id"].as_str().unwrap().to_string();

        // Preconditions.
        assert!(backend.advertiser.is_active(&stid).await);
        assert!(backend.manager.get_doc(&stid).is_some());
        assert!(backend.snapshot_store.exists(&stid));
        assert!(sql.get(&mount_id).is_some());

        // Hold the shared doc to simulate a post-unshare commit later.
        let shared_doc = backend.manager.get_doc(&stid).unwrap();

        backend.unshare(&mount_id).await.unwrap();

        assert!(
            !backend.advertiser.is_active(&stid).await,
            "advertiser still active after unshare"
        );
        assert!(
            backend.manager.get_doc(&stid).is_none(),
            "shared doc still registered after unshare"
        );
        assert!(
            !backend.snapshot_store.exists(&stid),
            "snapshot file not deleted by unshare"
        );
        assert!(
            sql.get(&mount_id).is_none(),
            "mount row still in SQL after unshare"
        );
        assert!(
            sql.get("block:shared-child").is_none(),
            "descendant row still in SQL after unshare"
        );

        // Mount node removed from the global tree.
        {
            let collab = backend.global_doc().await.unwrap();
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let bare = mount_id.strip_prefix("block:").unwrap();
            let uri = EntityUri::block(bare);
            assert!(
                find_tree_id_by_stable_id(doc, &uri).is_none(),
                "mount node still present in global tree"
            );
        }

        // gc-resurrection guard: a commit to the detached shared doc must not
        // recreate the snapshot — the save worker was aborted by unshare.
        {
            let tree = shared_doc.get_tree(crate::loro_backend::TREE_NAME);
            let node = tree.create(None::<TreeID>).unwrap();
            tree.get_meta(node).unwrap().insert("k", "v").unwrap();
            shared_doc.commit();
        }
        tokio::time::sleep(std::time::Duration::from_millis(400)).await;
        assert!(
            !backend.snapshot_store.exists(&stid),
            "snapshot resurrected after unshare (worker not dropped)"
        );
    }

    /// PRIVACY RUNG at prod altitude for `commit_share_prune`. Sharing moves a
    /// subtree out of the global doc; its blocks' content must move with it.
    /// The global doc's state is what a later share of an unrelated subtree is
    /// forked from.
    ///
    /// The assertion is on a shallow export, which is the boundary the purge
    /// reaches: it clears state, not the oplog. `LoroDocumentStore::save_all`
    /// writes a full snapshot on 63 of every 64 saves, so the on-disk file
    /// keeps the shared blocks' original ops until the next compaction (tasks
    /// #79/#80).
    #[tokio::test]
    async fn share_subtree_leaves_no_shared_plaintext_in_the_global_doc() {
        const SHARED_SECRET: &str = "SHARED-CHILD-SECRET-3d90";

        let (backend, _sql, _dir) = make_backend_with_sql();
        seed_block(&backend, "root-a", None, "root-a").await;
        seed_block(&backend, "shared-parent", Some("root-a"), "Shared heading").await;
        seed_block(
            &backend,
            "shared-child",
            Some("shared-parent"),
            SHARED_SECRET,
        )
        .await;

        backend
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();

        let collab = backend.global_doc().await.unwrap();
        let doc_arc = collab.doc();
        let doc = &*doc_arc;
        let bytes = doc
            .export(loro::ExportMode::shallow_snapshot(&doc.oplog_frontiers()))
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SHARED_SECRET),
            "the shared subtree's plaintext survived in the global doc's compacted state"
        );
    }

    /// `unshare` deletes the mount node, so whatever containers that node owns
    /// must go with it. Prod mount nodes carry only plain values today (writes
    /// to a mount are rejected — see `write_to_mount_node_rejects`), so the
    /// container here is written directly: this pins the call site's contract
    /// rather than a reachable leak.
    #[tokio::test]
    async fn unshare_takes_the_mount_nodes_own_containers_with_it() {
        const MOUNT_SECRET: &str = "MOUNT-NODE-SECRET-e402";

        let (backend, _sql, _dir) = make_backend_with_sql();
        seed_block(&backend, "root-a", None, "root-a").await;
        seed_block(&backend, "shared-parent", Some("root-a"), "Shared heading").await;

        let resp = backend
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();
        let json: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response: {other:?}"),
        };
        let mount_id = json["mount_block_id"].as_str().unwrap().to_string();

        {
            let collab = backend.global_doc().await.unwrap();
            let doc_arc = collab.doc();
            let doc = &*doc_arc;
            let bare = mount_id.strip_prefix("block:").unwrap();
            let mount_tid = find_tree_id_by_stable_id(doc, &EntityUri::block(bare))
                .expect("mount node must be in the global tree after share");
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            let meta = tree.get_meta(mount_tid).unwrap();
            let text: loro::LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
            text.insert(0, MOUNT_SECRET).unwrap();
            doc.commit();
        }

        backend.unshare(&mount_id).await.unwrap();

        let collab = backend.global_doc().await.unwrap();
        let doc_arc = collab.doc();
        let doc = &*doc_arc;
        let bytes = doc
            .export(loro::ExportMode::shallow_snapshot(&doc.oplog_frontiers()))
            .unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(MOUNT_SECRET),
            "the unshared mount node's container survived in the global doc's compacted state"
        );
    }
}

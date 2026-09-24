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
use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use holon_api::EntityName;
use holon_api::EntityUri;
use holon_api::OperationDescriptor;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_api::condition_bus::Condition;
use holon_api::condition_bus::ConditionBus;
use holon_api::condition_bus::ConditionKey;
use holon_api::condition_bus::ConditionKind;
use holon_api::sharing::Capabilities;
use holon_core::DownstreamProjection;
use holon_core::MaybeSendSync;
use holon_core::OperationProvider;
use holon_core::OperationResult;
use holon_core::OriginTaggedWrites;
use holon_core::Result;
use holon_core::UndoAction;
use holon_core::WriteTierAuthority;
use iroh::EndpointAddr;
use iroh::SecretKey;
use loro::LoroDoc;
use loro::TreeID;
use loro::TreeParentId;
use tokio::sync::RwLock;
use tokio::time::Duration;
use tokio::time::timeout;
use tracing::warn;
use uuid::Uuid;

use crate::debounced_commit_worker::DebouncedCommitWorkerHandle;
use crate::debounced_commit_worker::any_commit;
use crate::debounced_commit_worker::local_only;
use crate::debounced_commit_worker::{self};
use crate::iroh_advertiser::ALPN_PREFIX;
use crate::iroh_advertiser::IrohAdvertiser;
use crate::iroh_advertiser::OnPeerConnected;
use crate::iroh_advertiser::ShareAdmission;
use crate::iroh_advertiser::SharedRoster;
use crate::iroh_sync_adapter::SharedTreeSyncManager;
use crate::iroh_sync_adapter::create_endpoint;
use crate::iroh_sync_adapter::make_alpn;
use crate::iroh_sync_adapter::sync_doc_initiate_enrolled;
use crate::loro_document_store::DocScope;
use crate::loro_document_store::LoroDocumentStore;
use crate::owner_identity::OwnerIdentityKey;
use crate::peer_import::PeerReadAccess;
use crate::roster_sidecar::RosterSidecar;
use crate::roster_sidecar::RosterSidecarBody;
use crate::share_credentials::ShareCredentials;
use crate::share_enrollment::CapabilitySecret;
use crate::share_enrollment::ExpiryTime;
use crate::share_enrollment::PeerFingerprint;
use crate::share_enrollment::ShareRoster;
use crate::share_enrollment::peer_fingerprint;
use crate::share_peer_id::stable_peer_id;
use crate::shared_snapshot_store::SharedSnapshotStore;
use crate::shared_tree::HistoryRetention;
use crate::shared_tree::MountRole;
use crate::shared_tree::NestedShareRefusal;
use crate::shared_tree::SHARE_ROLE_MOUNT;
use crate::shared_tree::SHARE_ROLE_PROPERTY;
use crate::shared_tree::SHARED_TREE_ID_PROPERTY;
use crate::shared_tree::ShareKind;
use crate::shared_tree::{self};
use crate::ticket::Ticket;
use crate::write_origin::WriteOrigin;

fn err(msg: impl Into<String>) -> Box<dyn std::error::Error + Send + Sync> {
    Box::<dyn std::error::Error + Send + Sync>::from(msg.into())
}

/// The write-tier decision for one block this device imported from a peer.
///
/// This is the dispatcher's `OpOrigin::Sync` branch (`enforce_write_tier`),
/// placed where the import actually happens: the share projection legs write
/// straight to the SQL block provider, so nothing they do is ever judged by
/// the dispatcher. The import LANDS — the merge already happened in the peer,
/// and refusing it here would only make this store disagree with the replica —
/// and it inherits the tier of the document that owns its parent, or the
/// recipient is left with a block it can edit and no writer can ever put into
/// the authoritative file.
async fn adopt_imported_block(
    authority: Option<&Arc<dyn WriteTierAuthority>>,
    id: &str,
    parent_id: &str,
) -> Result<()> {
    // Absent only where the composition has no vault root, and disclosed once
    // by `new_with_sql` when it is — not swallowed here.
    let Some(authority) = authority else {
        return Ok(());
    };
    if !authority.any_read_only_documents() {
        return Ok(());
    }
    if authority
        .adopt_sync_import(id, parent_id)
        .await
        .map_err(|e| err(format!("write-tier adoption of imported block {id}: {e}")))?
    {
        warn!(
            block = %id,
            parent = %parent_id,
            "[LoroShareBackend] a sync import added a block under a read-only-homed document. It \
             is stored and it is uneditable: no writer can ever put it into the authoritative file."
        );
    }
    Ok(())
}

/// [`adopt_imported_block`] for a batch of projection ops. Mirrors the
/// dispatcher's own rule — every block write naming both a subject and a
/// destination is judged, whatever the op is called.
async fn adopt_imported_blocks(
    authority: Option<&Arc<dyn WriteTierAuthority>>,
    ops: &[(String, StorageEntity)],
) -> Result<()> {
    for (_, params) in ops {
        let (Some(id), Some(parent)) = (
            params.get("id").and_then(|v| v.as_string()),
            params.get("parent_id").and_then(|v| v.as_string()),
        ) else {
            continue;
        };
        adopt_imported_block(authority, id, parent).await?;
    }
    Ok(())
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
use crate::settled_read::LiveNode;
use crate::settled_read::classify;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default enrollment window for a share ticket's capability: after this many
/// seconds no *new* peer may enroll (already-enrolled peers keep syncing). 30
/// days is a placeholder pending the lease-policy ruling (ADR 0028 D4/H8).
const DEFAULT_ENROLLMENT_WINDOW_SECS: i64 = 30 * 24 * 60 * 60;
/// How many distinct peers one share's roster may pin. Bounds the blast radius
/// of a leaked ticket: the (max+1)-th holder is refused loudly rather than
/// quietly joining.
const DEFAULT_SHARE_MAX_PEERS: usize = 8;

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
///
/// Every id parameter here addresses a BLOCK: `tree` is the operation surface
/// the sharing machinery registers under, while the values it carries are
/// nodes of the block tree, so each is declared `#[entity_ref("block")]`.
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
    async fn share_subtree(
        &self,
        #[entity_ref("block")] id: &str,
        retention: String,
    ) -> Result<OperationResult>;

    /// Accept a shared subtree under `parent_id`, using a ticket generated
    /// by `share_subtree` on the other peer. Returns the new mount block's
    /// stable id via `OperationResult::response`.
    #[holon_macros::affects("parent_id")]
    async fn accept_shared_subtree(
        &self,
        #[entity_ref("block")] parent_id: &str,
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

    /// Stop sharing (on an owner) or leave (on a recipient) the share known by
    /// `id`: the page of a page share, the container row of a block share.
    ///
    /// Tears the share down in a resurrection-safe order: drop the per-share
    /// workers FIRST (so no worker can re-write the snapshot after we delete
    /// it), close the advertiser endpoint, unregister the shared doc, delete
    /// the mount node from the global tree plus the share's SQL rows, and
    /// finally delete the on-disk snapshot. Loud error if `id` names no share.
    #[holon_macros::affects("parent_id")]
    async fn unshare(&self, #[entity_ref("block")] id: &str) -> Result<OperationResult>;
}

/// Holder for a per-share save worker. Wraps the generic
/// [`DebouncedCommitWorkerHandle`] plus the `Arc<LoroDoc>` the worker
/// persists — `flush_all` needs the `doc` handle to force a final save
/// before shutdown without waiting for the debounce window.
struct SaveWorker {
    handle: DebouncedCommitWorkerHandle,
    doc: Arc<LoroDoc>,
}

/// How long to coalesce burst commits before writing to disk. Small
/// enough that a `SIGKILL` loses only a few keystrokes; large enough
/// that a typing burst produces one write, not hundreds.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(150);

fn spawn_save_worker(
    store: Arc<SharedSnapshotStore>,
    bus: Arc<ConditionBus>,
    shared_tree_id: String,
    doc: Arc<LoroDoc>,
) -> SaveWorker {
    let id_for_work = shared_tree_id.clone();
    let doc_for_work = doc.clone();
    let store_for_work = store.clone();
    let bus_for_work = bus.clone();
    let nested = Arc::new(std::sync::Mutex::new(NestedMountWatch::new()));
    let nested_for_work = nested.clone();
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
            nested_for_work.lock().unwrap().check(&bus, &id, &doc);
            async move {
                if let Err(e) = store.save(&id, &doc) {
                    // Emit the degraded-mode signal for the UI. The
                    // `Err` return also surfaces in the worker's
                    // tracing::error so both the bus-listener and the
                    // operator log the same failure — no swallowing.
                    bus.emit(Condition {
                        subject: id.clone(),
                        reason: ConditionKind::SnapshotSaveFailed(format!("{e:#}")),
                    });
                    return Err(Box::<dyn std::error::Error + Send + Sync>::from(format!(
                        "snapshot save for {id} failed: {e:#}"
                    )));
                }
                // The save condition's all-clear: the snapshot is on disk, so a
                // banner from a previous failure is no longer true.
                bus.clear(&ConditionKey {
                    subject: id.clone(),
                    kind: ConditionKind::SNAPSHOT_SAVE_FAILED,
                });
                Ok(())
            }
        },
    );
    // After the subscription, so an import that lands before it is still seen.
    nested.lock().unwrap().check(&bus, &shared_tree_id, &doc);

    SaveWorker { handle, doc }
}

/// Backing state for subtree share operations. Kept separate from
/// `LoroBackend` (which has many other responsibilities) so the iroh
/// endpoint lifecycle is isolated.
pub struct LoroShareBackend {
    store: Arc<RwLock<LoroDocumentStore>>,
    snapshot_store: Arc<SharedSnapshotStore>,
    manager: Arc<SharedTreeSyncManager>,
    advertiser: Arc<IrohAdvertiser>,
    degraded_bus: Arc<ConditionBus>,
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
    /// The write-tier authority, for the blocks a peer's import puts under a
    /// document homed in a read-only format.
    ///
    /// The projection legs below write straight to `sql_ops`, never through
    /// the operation dispatcher, so the dispatcher's `OpOrigin::Sync` branch
    /// never judges them. This field is that branch, held where the import
    /// actually happens. `Option` for the same reason as `sql_ops`: tests
    /// without the DI stack skip it.
    write_tier: Option<Arc<dyn WriteTierAuthority>>,
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
    /// Custody of every share's capability secret plus the owner key that
    /// signs its roster sidecar. This is what makes a share GATED: without a
    /// capability there is no roster, and without a roster the advertiser
    /// would admit whoever reaches the endpoint.
    credentials: Arc<ShareCredentials>,
    /// Weak reference to self, populated during `Arc::new_cyclic`
    /// construction — the closure receives a `&Weak<Self>` BEFORE the
    /// `Arc` is fully built, so the field is set up once atomically.
    /// Internal callbacks (e.g. the advertiser's on-peer-connected
    /// hook, the per-share auto-resync worker) upgrade this weak ref
    /// to call back into `&self`-shaped methods without threading
    /// `Arc<Self>` through every call site.
    self_weak: std::sync::Weak<LoroShareBackend>,
    /// Shares an accept is placing right now. An accept claims its share
    /// before its first side effect, so a concurrent second accept of the same
    /// ticket is refused before it binds, bumps or writes anything.
    accepting: Arc<std::sync::Mutex<HashSet<String>>>,
}

/// An accept's claim on its share, released when the accept ends.
struct AcceptClaim {
    accepting: Arc<std::sync::Mutex<HashSet<String>>>,
    shared_tree_id: String,
}

impl Drop for AcceptClaim {
    fn drop(&mut self) {
        self.accepting
            .lock()
            .expect("accept claims lock")
            .remove(&self.shared_tree_id);
    }
}

/// Which per-share writers a settle waits for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettleScope {
    /// Save and projection workers. Excludes the sync worker, whose
    /// `sync_with_peers` dials every known peer under `CONNECT_TIMEOUT`
    /// each — waiting on it prices the settle in peer reachability
    /// rather than in pending local work.
    LocalWrites,
    /// Also the sync worker. Costs a network round trip, and is what a
    /// caller needs when nothing may rewrite the snapshot afterwards:
    /// `sync_with_peers` republishes it as a save-before-push barrier
    /// before it dials.
    IncludingSync,
}

/// Holder for a per-share auto-resync worker. Wraps the generic
/// [`DebouncedCommitWorkerHandle`] — no per-worker state beyond the
/// handle itself (unlike `SaveWorker`, which keeps the `doc` handle
/// alive for `flush_all`).
struct SyncWorker {
    handle: DebouncedCommitWorkerHandle,
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

    SyncWorker { handle }
}

/// Per-share SQL projection worker. On every change to the shared doc, and on
/// every change to this device's placement of it (a move of its mount in the
/// global doc), diffs the current state against a watermark and projects
/// creates/updates/deletes into the SQL block table. Debounced.
struct ProjectionWorker {
    /// Woken by the shared doc's commits.
    content: DebouncedCommitWorkerHandle,
    /// Woken by the global doc's commits, which is where the mount moves.
    placement: DebouncedCommitWorkerHandle,
}

impl ProjectionWorker {
    fn quiesces(&self) -> [crate::debounced_commit_worker::WorkerQuiesce; 2] {
        [self.content.quiesce(), self.placement.quiesce()]
    }
}

/// Debounce for the SQL projection worker. Same cadence as save —
/// every local or imported change should become visible in the UI
/// quickly, but not so quickly that a typing burst floods the DB.
const PROJECTION_DEBOUNCE: Duration = Duration::from_millis(150);

/// Where a share's root sits in this device's SQL projection.
#[derive(Clone, Debug, PartialEq, Eq)]
enum RootPlacement {
    /// Page share: the page's OWN row, hung where the mount sits. The mount is
    /// this device's placement record for the page (Overlay proposal §2), so
    /// it lends the row its parent and position and nothing else.
    Page { parent: EntityUri, sort_key: String },
    /// Block share: the shared block sits under the synthetic container page
    /// the mount projects as.
    Container { mount: EntityUri },
}

impl RootPlacement {
    /// Hang the shared doc's root where this placement says, and stamp every
    /// block with the share it belongs to.
    fn apply(
        &self,
        blocks: &mut HashMap<String, crate::loro_backend::SnapshotBlock>,
        shared_tree_id: &str,
    ) {
        for snap in blocks.values_mut() {
            snap.block
                .properties
                .entry(SHARED_TREE_ID_PROPERTY.to_string())
                .or_insert_with(|| Value::String(shared_tree_id.to_string()));
            if !(snap.block.parent_id.is_no_parent() || snap.block.parent_id.is_sentinel()) {
                continue;
            }
            match self {
                RootPlacement::Page { parent, sort_key } => {
                    snap.block.parent_id = parent.clone();
                    snap.sort_key = sort_key.clone();
                }
                RootPlacement::Container { mount } => snap.block.parent_id = mount.clone(),
            }
        }
    }
}

/// One share as this device holds it: the mount that places it and whether its
/// root is a page. Both are fixed for the share's life on this device — a
/// mount's `TreeID` survives every move, and the kind is recorded on the mount
/// when the share is made — so the placement itself is the only part re-read
/// per pass.
#[derive(Clone, Copy, Debug)]
struct ShareRoot {
    mount: TreeID,
    root_is_page: bool,
}

impl ShareRoot {
    /// Locate the mount of `shared_tree_id` in the global doc and read the
    /// share kind recorded on it.
    fn locate(global: &LoroDoc, shared_tree_id: &str) -> anyhow::Result<Self> {
        let tree = global.get_tree(crate::loro_backend::TREE_NAME);
        let mount = shared_tree::find_mount_node(&tree, shared_tree_id).ok_or_else(|| {
            anyhow::anyhow!(
                "shared tree {shared_tree_id} has no mount in this device's tree, so nothing \
                 places its root"
            )
        })?;
        let info = shared_tree::read_mount_info(&tree, mount)
            .ok_or_else(|| anyhow::anyhow!("mount node {mount:?} carries no mount metadata"))?;
        Ok(Self {
            mount,
            root_is_page: matches!(info.kind()?, ShareKind::Page { .. }),
        })
    }

    /// The placement the mount currently gives the root.
    fn placement(&self, global: &LoroDoc) -> anyhow::Result<RootPlacement> {
        let tree = global.get_tree(crate::loro_backend::TREE_NAME);
        if !self.root_is_page {
            let mount = read_stable_id(&tree, self.mount).ok_or_else(|| {
                anyhow::anyhow!("mount node {:?} has no readable STABLE_ID", self.mount)
            })?;
            return Ok(RootPlacement::Container {
                mount: EntityUri::parse(&block_uri_from_bare(&mount))?,
            });
        }
        Ok(RootPlacement::Page {
            parent: crate::loro_backend::node_parent_uri(&tree, self.mount)?,
            sort_key: crate::loro_backend::node_sort_key(&tree, self.mount).ok_or_else(|| {
                anyhow::anyhow!(
                    "mount node {:?} has no fractional index, so the page it places has no \
                     position",
                    self.mount
                )
            })?,
        })
    }
}

/// What a projection pass last wrote: the shared doc's frontiers and the
/// placement its root was written under.
struct ProjectionMark {
    frontiers: loro::Frontiers,
    placement: RootPlacement,
}

/// One share's Loro→SQL projection. Two workers drive it (shared-doc commits
/// and global-doc commits); the mark's lock serializes their passes.
struct ShareProjection {
    doc: Arc<LoroDoc>,
    sql_ops: Arc<dyn OriginTaggedWrites>,
    bus: Arc<ConditionBus>,
    shared_tree_id: String,
    root: ShareRoot,
    global_doc: Arc<crate::loro_document::LoroDocument>,
    write_tier: Option<Arc<dyn WriteTierAuthority>>,
    mark: tokio::sync::Mutex<ProjectionMark>,
}

type WorkResult = std::result::Result<(), Box<dyn std::error::Error + Send + Sync>>;

impl ShareProjection {
    fn placement_now(
        &self,
    ) -> std::result::Result<RootPlacement, Box<dyn std::error::Error + Send + Sync>> {
        let stid = &self.shared_tree_id;
        self.global_doc
            .with_read(|g| self.root.placement(g))
            .map_err(|e| format!("shared doc {stid}: read the root's placement: {e:#}").into())
    }

    async fn pass(&self) -> WorkResult {
        use crate::loro_backend::snapshot_blocks_from_doc;
        use crate::loro_sync_controller::is_empty_frontiers;

        let stid = self.shared_tree_id.as_str();
        let mut mark = self.mark.lock().await;
        let current = self.doc.oplog_frontiers();
        let placement = self.placement_now()?;
        if mark.frontiers == current && mark.placement == placement {
            return Ok(());
        }

        let before = if is_empty_frontiers(&mark.frontiers) {
            HashMap::new()
        } else {
            let fork = self.doc.fork_at(&mark.frontiers).map_err(|e| {
                format!("shared doc projection for {stid}: fork_at watermark failed: {e}")
            })?;
            let mut snap = snapshot_blocks_from_doc(&fork);
            mark.placement.apply(&mut snap, stid);
            snap
        };

        let (ops, after_settled) = share_diff_ops(&self.doc, &before, &placement, stid);
        if !ops.is_empty() {
            // Integrity guard (see `first_local_collision`): a synced-in
            // remote edit must never project a block whose id shadows a
            // LIVE local block. Refuse loudly (banner + `Err` freezes the
            // watermark) instead of letting the remote clobber the
            // recipient's own SQL row.
            if let Some(bad) = self
                .global_doc
                .with_read(|d| Ok(first_local_collision(d, &ops)))
                .map_err(|e| {
                    format!("shared doc {stid}: global-doc collision-guard read failed: {e:#}")
                })?
            {
                self.bus.emit(Condition {
                    subject: stid.to_string(),
                    reason: ConditionKind::ForeignIdCollision(bad.clone()),
                });
                return Err(format!(
                    "shared doc {stid} projection refused: block id {bad:?} collides with a live \
                     local block — refusing to clobber local content"
                )
                .into());
            }
            adopt_imported_blocks(self.write_tier.as_ref(), &ops).await?;
            let entity = EntityName::new("block");
            // SECURITY (Ruling B): shared-doc projection ops carry
            // `position: None` — the Loro tree owns order, this sink
            // never mints re-keys, so a peer block's `_order_rekeys`
            // property is structurally unable to reach the re-key channel.
            let batch: Vec<holon_core::BatchOp> = ops
                .into_iter()
                .map(|(op_name, params)| holon_core::BatchOp::data(op_name, params))
                .collect();
            if let Err(e) = self
                .sql_ops
                .execute_batch_with_origin(&entity, batch, crate::event_bus::EventOrigin::Loro)
                .await
            {
                // Loro accepted the change but SQL rejected the projection —
                // the two now diverge and the UI (which reads SQL) is stale.
                // Surface a banner AND return `Err` so the mark stays put and
                // the next commit retries.
                self.bus.emit(Condition {
                    subject: stid.to_string(),
                    reason: ConditionKind::SqlProjectionFailed(format!("{e:#}")),
                });
                return Err(format!("shared doc projection for {stid} failed: {e:#}").into());
            }
        }

        // Reaching here means the collision guard passed AND SQL took the
        // batch — the all-clear for both projection conditions.
        for kind in [
            ConditionKind::SQL_PROJECTION_FAILED,
            ConditionKind::FOREIGN_ID_COLLISION,
        ] {
            self.bus.clear(&ConditionKey {
                subject: stid.to_string(),
                kind,
            });
        }

        mark.placement = placement;
        if after_settled {
            mark.frontiers = current;
        } else {
            // Freezing the frontiers keeps the pre-mutation base: withheld
            // deletes (including LEGITIMATE ones that happened to land in an
            // unsettled pass) are re-diffed and emitted on the next settled
            // pass. Advancing it here would drop them permanently.
            tracing::warn!(
                "shared doc {stid}: snapshot unsettled — watermark frozen; deletes withheld \
                 until the next settled projection pass"
            );
        }
        Ok(())
    }
}

/// Spawn the projection of one share. The mark starts at the doc's current
/// frontiers and the root's current placement, so the first pass projects only
/// what changes after the caller's own initial projection.
fn spawn_projection_worker(
    doc: Arc<LoroDoc>,
    sql_ops: Arc<dyn OriginTaggedWrites>,
    bus: Arc<ConditionBus>,
    shared_tree_id: String,
    root: ShareRoot,
    global_doc: Arc<crate::loro_document::LoroDocument>,
    write_tier: Option<Arc<dyn WriteTierAuthority>>,
) -> Result<ProjectionWorker> {
    let placement = global_doc.with_read(|g| root.placement(g)).map_err(|e| {
        err(format!(
            "shared doc {shared_tree_id}: read the root's placement: {e:#}"
        ))
    })?;
    // ALLOW(loro_doc_escape): a subscription registration — the placement
    // worker only listens for global commits; every read goes through `with_read`.
    let global_raw = global_doc.doc();
    let projection = Arc::new(ShareProjection {
        mark: tokio::sync::Mutex::new(ProjectionMark {
            frontiers: doc.oplog_frontiers(),
            placement,
        }),
        doc: doc.clone(),
        sql_ops,
        bus,
        shared_tree_id,
        root,
        global_doc,
        write_tier,
    });
    let driver = |projection: Arc<ShareProjection>| {
        move || {
            let projection = projection.clone();
            async move { projection.pass().await }
        }
    };
    Ok(ProjectionWorker {
        content: debounced_commit_worker::spawn(
            doc,
            any_commit(),
            PROJECTION_DEBOUNCE,
            "share.project",
            driver(projection.clone()),
        ),
        placement: debounced_commit_worker::spawn(
            global_raw,
            any_commit(),
            PROJECTION_DEBOUNCE,
            "share.place",
            driver(projection),
        ),
    })
}

/// One share-projection diff step: snapshot `after` settled-aware, apply the
/// share's placement, diff against `before`, and WITHHOLD delete ops when the
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
    placement: &RootPlacement,
    stid: &str,
) -> (Vec<(String, StorageEntity)>, bool) {
    use crate::loro_backend::snapshot_blocks_from_doc_settled;
    use crate::loro_sync_controller::diff_snapshots_to_ops;
    let (mut after, after_settled) = snapshot_blocks_from_doc_settled(doc);
    placement.apply(&mut after, stid);
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
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        store: Arc<RwLock<LoroDocumentStore>>,
        snapshot_store: Arc<SharedSnapshotStore>,
        manager: Arc<SharedTreeSyncManager>,
        advertiser: Arc<IrohAdvertiser>,
        degraded_bus: Arc<ConditionBus>,
        device_key: SecretKey,
        credentials: Arc<ShareCredentials>,
    ) -> Arc<Self> {
        Self::new_with_sql(
            store,
            snapshot_store,
            manager,
            advertiser,
            degraded_bus,
            device_key,
            credentials,
            None,
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
        degraded_bus: Arc<ConditionBus>,
        device_key: SecretKey,
        credentials: Arc<ShareCredentials>,
        sql_ops: Option<Arc<dyn OriginTaggedWrites>>,
        downstream_projection: Option<Arc<dyn DownstreamProjection>>,
        write_tier: Option<Arc<dyn WriteTierAuthority>>,
    ) -> Arc<Self> {
        // Disclosed ONCE, here, rather than per write: a backend that projects
        // into SQL but cannot ask the tier will store a peer's blocks under a
        // read-only-homed document as fully editable, and nothing can ever put
        // them into the authoritative file. `holon-app`'s wiring registers the
        // authority only inside its vault-root branch, so a session with no
        // vault root legitimately has none — and no files either, which is why
        // this is a disclosure and not a refusal.
        if sql_ops.is_some() && write_tier.is_none() {
            warn!(
                "[LoroShareBackend] constructed with a SQL projection sink but no \
                 WriteTierAuthority: sync imports landing under a read-only-homed document will \
                 be stored EDITABLE and no writer can ever put them into the authoritative file. \
                 Expected only in a session with no vault root, which has no such documents."
            );
        }
        let backend = Arc::new_cyclic(|self_weak| Self {
            store,
            snapshot_store,
            manager,
            advertiser,
            degraded_bus,
            device_key,
            credentials,
            sql_ops,
            downstream_projection,
            write_tier,
            known_peers: Arc::new(RwLock::new(HashMap::new())),
            save_workers: Arc::new(RwLock::new(HashMap::new())),
            sync_workers: Arc::new(RwLock::new(HashMap::new())),
            projection_workers: Arc::new(RwLock::new(HashMap::new())),
            self_weak: self_weak.clone(),
            accepting: Arc::new(std::sync::Mutex::new(HashSet::new())),
        });
        let exit: std::sync::Weak<dyn shared_tree::ShareExit> = Arc::downgrade(&backend) as _;
        backend.manager.set_exit(exit);
        backend
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
    /// doc. On every change to the doc or to this device's placement of it,
    /// diffs against a watermark and writes the delta into the SQL block table.
    /// No-op when `sql_ops` is `None` (tests without the DI stack).
    pub async fn attach_projection_worker(
        &self,
        shared_tree_id: String,
        doc: Arc<LoroDoc>,
    ) -> Result<()> {
        let Some(sql_ops) = self.sql_ops.as_ref() else {
            return Ok(());
        };
        let global_doc = self.global_doc().await?;
        let root = global_doc.with_read(|g| ShareRoot::locate(g, &shared_tree_id))?;
        let worker = spawn_projection_worker(
            doc,
            sql_ops.clone(),
            self.degraded_bus.clone(),
            shared_tree_id.clone(),
            root,
            global_doc,
            self.write_tier.clone(),
        )?;
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
                self.degraded_bus.emit(Condition {
                    subject: id,
                    reason: ConditionKind::SnapshotSaveFailed(format!("{e:#}")),
                });
            }
        }
    }

    /// Resolve once the per-share workers in `scope` have no pending
    /// debounce window and no work call in flight. Iterates to a fixed
    /// point because a commit re-arms them all. Never times out; bound
    /// it at the call site.
    ///
    /// Rehydration's kick-sync is a detached task and is outside every
    /// scope, so a caller inspecting `shares/` must re-settle and
    /// re-check rather than treat one pass as final.
    pub async fn wait_for_workers_idle(&self, scope: SettleScope) {
        loop {
            let quiesces = {
                let save = self.save_workers.read().await;
                let projection = self.projection_workers.read().await;
                let sync = self.sync_workers.read().await;
                save.values()
                    .map(|w| w.handle.quiesce())
                    .chain(projection.values().flat_map(|w| w.quiesces()))
                    .chain(
                        sync.values()
                            .filter(|_| scope == SettleScope::IncludingSync)
                            .map(|w| w.handle.quiesce()),
                    )
                    .collect::<Vec<_>>()
            };
            if quiesces.iter().all(|q| q.is_idle()) {
                return;
            }
            for q in &quiesces {
                q.wait_idle().await;
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
    pub fn degraded_bus(&self) -> &Arc<ConditionBus> {
        &self.degraded_bus
    }

    /// Project a BLOCK share's synthetic container page into the SQL `block`
    /// table: the mount's own row, titled after the share and tagged `Page` so
    /// the shared block's write-back owns a file (Amendment A parents the mount
    /// under a page). A PAGE share has no such row — its page is its own row
    /// (see [`RootPlacement`]).
    ///
    /// Uses the SQL operation provider's `create` op, an UPSERT.
    /// No-op when `sql_ops` is `None` (backend-only tests).
    async fn project_container_to_sql(
        &self,
        mount_block_uri: &str,
        parent_block_uri: &str,
        shared_tree_id: &str,
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
        params.insert(
            "content".into(),
            Value::String(format!("Shared tree ({shared_tree_id})")),
        );
        params.insert("content_type".into(), Value::String("text".to_string()));
        params.insert(
            "tags".into(),
            Value::Array(vec![Value::String(holon_api::block::PAGE_TAG.to_string())]),
        );
        // Custom properties — `SqlOperationProvider::prepare_create` packs
        // any key not in the schema-derived block columns into the `properties` JSON.
        params.insert(
            SHARE_ROLE_PROPERTY.into(),
            Value::String(SHARE_ROLE_MOUNT.to_string()),
        );
        params.insert(
            SHARED_TREE_ID_PROPERTY.into(),
            Value::String(shared_tree_id.to_string()),
        );

        // A share accepted UNDER a block of a read-only-homed document puts the
        // container itself inside that document.
        adopt_imported_block(self.write_tier.as_ref(), mount_block_uri, parent_block_uri).await?;

        let entity = EntityName::new("block");
        sql_ops
            .execute_operation(&entity, "create", params)
            .await
            .map_err(|e| err(format!("project the share's container page into SQL: {e}")))?;
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

    /// Project all nodes from a shared LoroDoc into SQL block rows, the root
    /// placed where its mount says (see [`RootPlacement`]) and every row
    /// stamped with `shared-tree-id` so downstream queries and routing can
    /// tell which share it belongs to.
    ///
    /// The `create` op is an UPSERT, so this is idempotent across accept and
    /// rehydrate and refreshes rows an earlier session left behind.
    async fn project_descendants_to_sql(
        &self,
        shared_doc: &LoroDoc,
        shared_tree_id: &str,
    ) -> Result<()> {
        let Some(sql_ops) = self.sql_ops.as_ref() else {
            return Ok(());
        };
        let global = self.global_doc().await?;
        let placement = global
            .with_read(|g| ShareRoot::locate(g, shared_tree_id)?.placement(g))
            .map_err(|e| err(format!("place shared tree {shared_tree_id}'s root: {e:#}")))?;
        let mut blocks = crate::loro_backend::snapshot_blocks_from_doc(shared_doc);
        placement.apply(&mut blocks, shared_tree_id);
        let ops = crate::loro_sync_controller::diff_snapshots_to_ops(&HashMap::new(), &blocks);
        if ops.is_empty() {
            return Ok(());
        }
        // Integrity guard (see `first_local_collision`): a shared doc must never
        // project a block whose id shadows a LIVE local block. This runs at
        // accept/re-project time; a collision means a hostile or corrupt shared
        // doc, so reject the whole projection loudly rather than clobber local
        // SQL rows.
        if let Some(bad) = global
            .with_read(|d| Ok(first_local_collision(d, &ops)))
            .map_err(|e| err(format!("global-doc read for collision guard: {e:#}")))?
        {
            return Err(err(format!(
                "shared doc {shared_tree_id} projection refused: block id {bad:?} collides with a \
                 live local block — refusing to shadow local content"
            )));
        }
        adopt_imported_blocks(self.write_tier.as_ref(), &ops).await?;
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
        Ok(collab.with_read(|doc| {
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            for node in tree.get_nodes(false) {
                if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
                    continue;
                }
                let Some(info) = shared_tree::read_mount_info(&tree, node.id) else {
                    continue;
                };
                if let Some(sid) = read_stable_id(&tree, node.id)
                    && sid.strip_prefix("block:").unwrap_or(&sid) == bare
                {
                    return Ok(true);
                }
                // A page share's file is the page's own: its document id is
                // the shared root's, placed by this mount.
                if let shared_tree::KindRecord::Recorded(ShareKind::Page { root }) = &info.kind
                    && root == bare
                {
                    return Ok(true);
                }
            }
            Ok(false)
        })?)
    }

    async fn global_doc(&self) -> Result<Arc<crate::loro_document::LoroDocument>> {
        let store = self.store.read().await;
        store
            .get_doc(DocScope::Global)
            .await
            .map_err(|e| err(format!("get_doc(Global) failed: {e:#}")))
    }

    /// Record an INBOUND dialer as a peer of `access`'s container. Takes the
    /// read witness because `sync_with_peers` later dials everything this
    /// writes, granting `Capabilities::read_write()` — so a peer the admission
    /// refused must not be able to get in here.
    async fn remember_admitted_peer(&self, access: &PeerReadAccess, addr: EndpointAddr) {
        self.remember_peer(access.container(), addr).await;
    }

    /// The store mutation itself. Private, and reachable from exactly two
    /// bases: an inbound dialer the admission accepted
    /// ([`Self::remember_admitted_peer`]) and the ticket author's own addr,
    /// which THIS device chose to accept a share from.
    ///
    /// The sidecar write happens UNDER the `known_peers` guard, not after it:
    /// a revocation persisting the narrowed set runs concurrently, and two
    /// writers that each snapshot the map and then race to disk let the stale
    /// snapshot land last — putting a revoked peer's addr back while both
    /// report success.
    async fn remember_peer(&self, shared_tree_id: &str, addr: EndpointAddr) {
        let persist = {
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
            let persisted = entry.clone();
            self.snapshot_store.save_peers(shared_tree_id, &persisted)
        };
        if let Err(e) = persist {
            // Not fatal — the in-memory entry is authoritative while
            // the process runs. Surface as degraded so the user knows
            // cross-peer sync after restart may regress.
            warn!(
                shared_tree_id = %shared_tree_id,
                error = %e,
                "[share] save_peers failed"
            );
            self.degraded_bus.emit(Condition {
                subject: shared_tree_id.to_string(),
                reason: ConditionKind::SnapshotSaveFailed(format!(
                    "peers sidecar save failed: {e:#}"
                )),
            });
        }
    }

    /// Drop every remembered dial address belonging to `peer`, in memory and
    /// in the sidecar. `EndpointAddr::id` IS the node public key the roster
    /// pins, so the two identify the same principal.
    ///
    /// A sidecar that cannot be rewritten is an `Err`, not a warning: the
    /// in-memory drop dies with the process, so a revocation whose sidecar
    /// write failed is a revocation the next restart silently undoes — the
    /// peer's addr comes back and `sync_with_peers` dials it again. The caller
    /// must see that its revocation did not hold.
    ///
    /// Persisted under the `known_peers` guard for the reason spelled out on
    /// [`Self::remember_peer`]: an admission racing this write must not put the
    /// addr back.
    async fn forget_peer_addrs(&self, shared_tree_id: &str, peer: &PeerFingerprint) -> Result<()> {
        let persist = {
            let mut guard = self.known_peers.write().await;
            let Some(entry) = guard.get_mut(shared_tree_id) else {
                return Ok(());
            };
            entry.retain(|addr| &PeerFingerprint::from_bytes(*addr.id.as_bytes()) != peer);
            let remaining = entry.clone();
            self.snapshot_store.save_peers(shared_tree_id, &remaining)
        };
        persist.map_err(|e| {
            err(format!(
                "the peers sidecar for share {shared_tree_id} could not be rewritten after \
                 revoking peer {peer:?}, so that peer's dial addr survives on disk and comes \
                 back at the next restart: {e:#}"
            ))
        })
    }

    /// Build a peer-connected callback that remembers every inbound
    /// dialer the admission accepted. Returns an `OnPeerConnected`
    /// suitable for `IrohAdvertiser::start_share_with_callback`.
    fn peer_connected_callback(&self) -> OnPeerConnected {
        let weak = self.weak_self();
        Arc::new(move |access: &PeerReadAccess, addr: EndpointAddr| {
            let Some(strong) = weak.upgrade() else {
                return;
            };
            let access = access.clone();
            tokio::spawn(async move {
                strong.remember_admitted_peer(&access, addr).await;
                // The callback fires only AFTER the admission decision, so a
                // peer newly pinned by `acceptor_enroll` is in the live roster
                // by now. Writing the sidecar here is what lets it reconnect
                // after a restart without re-proving the capability.
                if let Err(e) = strong.persist_roster(access.container()).await {
                    warn!(
                        shared_tree_id = %access.container(),
                        error = %e,
                        "[share] roster sidecar not updated after an admission; a peer admitted \
                         now may have to re-enroll after a restart"
                    );
                }
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
    /// Build the acceptor roster for a share whose capability this device has
    /// just minted (author) or just received in a ticket (recipient), and
    /// write the signed sidecar a later restart rebuilds it from.
    ///
    /// The capability is filed in the keychain FIRST: a roster that exists
    /// only in memory would not survive a restart, and the restart path
    /// refuses to advertise a share it cannot gate.
    async fn install_roster(
        &self,
        shared_tree_id: &str,
        capability: &CapabilitySecret,
        expires_at: ExpiryTime,
    ) -> Result<SharedRoster> {
        self.credentials
            .store_capability(shared_tree_id, capability)
            .map_err(|e| err(format!("{e:#}")))?;
        let owner = self
            .credentials
            .owner_key(&self.degraded_bus)
            .map_err(|e| err(format!("{e:#}")))?;
        let roster = ShareRoster::new(
            shared_tree_id,
            capability.clone(),
            expires_at,
            DEFAULT_SHARE_MAX_PEERS,
        )
        .with_owner(owner.public());
        self.write_roster_sidecar(&roster, &owner)
            .map_err(|e| err(format!("{e:#}")))?;
        Ok(Arc::new(tokio::sync::Mutex::new(roster)))
    }

    /// Rebuild a share's roster after a restart: the capability from the
    /// keychain, the pinned-peer set from the owner-signed sidecar. An already
    /// -enrolled peer reconnects without re-proving, which is the whole point
    /// of persisting the pinned set.
    ///
    /// Every failure is loud. A share whose capability or sidecar is gone must
    /// not be advertised at all — advertising it un-gated is the hole.
    async fn rehydrate_roster(&self, shared_tree_id: &str) -> Result<SharedRoster> {
        let capability = self
            .credentials
            .load_capability(shared_tree_id)
            .map_err(|e| err(format!("{e:#}")))?;
        let owner = self
            .credentials
            .owner_key(&self.degraded_bus)
            .map_err(|e| err(format!("{e:#}")))?;
        let body = RosterSidecar::load(
            self.snapshot_store.shares_dir(),
            shared_tree_id,
            &owner.public(),
        )
        .map_err(|e| err(format!("{e:#}")))?;
        Ok(Arc::new(tokio::sync::Mutex::new(
            RosterSidecar::into_roster(body, capability),
        )))
    }

    fn write_roster_sidecar(
        &self,
        roster: &ShareRoster,
        owner: &OwnerIdentityKey,
    ) -> anyhow::Result<()> {
        // The B1 owner-signed device entries are a self-device fleet concern;
        // a third-party subtree share admits by capability only, so the entry
        // set is empty and the sidecar's `owner` field serves solely as the
        // integrity key.
        let body = RosterSidecarBody::from_roster(roster, Vec::new())?;
        RosterSidecar::save(self.snapshot_store.shares_dir(), &body, owner)
    }

    /// Snapshot the live roster's pinned-peer set back into the signed
    /// sidecar, so a peer admitted this session is still pinned after a
    /// restart. Called after every inbound admission and after a revocation.
    async fn persist_roster(&self, shared_tree_id: &str) -> Result<()> {
        let Some(roster) = self.advertiser.roster_for(shared_tree_id).await else {
            return Err(err(format!(
                "share {shared_tree_id} is advertised without a roster, so there is no pinned-peer \
                 set to persist"
            )));
        };
        let owner = self
            .credentials
            .owner_key(&self.degraded_bus)
            .map_err(|e| err(format!("{e:#}")))?;
        let guard = roster.lock().await;
        self.write_roster_sidecar(&guard, &owner).map_err(|e| {
            err(format!(
                "persist roster sidecar for {shared_tree_id}: {e:#}"
            ))
        })
    }

    /// Revoke one peer from a live share: un-pin it, close the enrollment
    /// window so the capability it may still hold cannot re-admit it, and
    /// persist both. Its next dial is refused at the enrollment gate, so no
    /// further delta of its reaches this device's copy.
    ///
    /// Returns whether the peer had been enrolled. An `Err` means the
    /// revocation did NOT fully hold — the roster sidecar or the peers sidecar
    /// could not be rewritten, so a restart brings the revoked peer back.
    pub async fn revoke_share_peer(
        &self,
        shared_tree_id: &str,
        peer: PeerFingerprint,
    ) -> Result<bool> {
        let Some(roster) = self.advertiser.roster_for(shared_tree_id).await else {
            return Err(err(format!(
                "share {shared_tree_id} is not advertised with a roster; there is nothing to \
                 revoke from"
            )));
        };
        let was_enrolled = {
            let mut guard = roster.lock().await;
            let removed = guard.revoke(&peer);
            guard.close_enrollment(chrono::Utc::now().timestamp());
            removed
        };
        self.persist_roster(shared_tree_id).await?;
        // Revoking only the acceptor side would leave the outbound leg intact:
        // `sync_with_peers` dials every remembered addr and pulls, so a revoked
        // peer's ops would keep arriving through OUR dial. Forget its addrs.
        self.forget_peer_addrs(shared_tree_id, &peer).await?;
        warn!(
            shared_tree_id = %shared_tree_id,
            peer = ?peer,
            was_enrolled = was_enrolled,
            "[share] peer revoked; the enrollment window is closed so its capability cannot \
             re-admit it"
        );
        Ok(was_enrolled)
    }

    async fn start_advertising_stable(
        &self,
        shared_tree_id: &str,
        doc: Arc<LoroDoc>,
        admission: ShareAdmission,
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
                admission,
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
            self.degraded_bus.emit(Condition {
                subject: shared_tree_id.to_string(),
                reason: ConditionKind::SnapshotSaveFailed(format!("pre-push save failed: {e:#}")),
            });
            return Err(err(format!(
                "pre-push snapshot save failed; refusing to push un-persisted ops: {e:#}"
            )));
        }
        // The peer on the other end gates us exactly as we gate it, so every
        // outbound round proves our own membership. A share whose capability
        // is gone cannot dial at all — the alternative would be an un-enrolled
        // dial that only works against a peer that gates nobody.
        let capability = self
            .credentials
            .load_capability(shared_tree_id)
            .map_err(|e| err(format!("cannot sync share {shared_tree_id}: {e:#}")))?;
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
            // `read_write` is what THIS device grants the peer it dialed, and
            // it is symmetric with what our own gated share confers on the
            // peers it admits: a subtree share is a shared workspace, not a
            // publication (D72.a — full writer after enforcement).
            let fut = sync_doc_initiate_enrolled(
                &ep,
                &doc,
                &alpn_bytes,
                addr,
                &capability,
                shared_tree_id,
                Capabilities::read_write(),
            );
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

    /// The loaded shared tree whose doc holds `id`.
    fn shared_tree_holding(&self, id: &EntityUri) -> Option<String> {
        use crate::shared_tree::SharedTreeStore;
        self.manager
            .shared_tree_ids()
            .into_iter()
            .find(|shared_tree_id| {
                self.manager
                    .get_doc(shared_tree_id)
                    .is_some_and(|doc| find_tree_id_by_stable_id(&doc, id).is_some())
            })
    }

    /// Test-only access to the shared-tree manager (to fetch shared docs).
    pub fn manager_for_test(&self) -> Arc<SharedTreeSyncManager> {
        self.manager.clone()
    }

    /// Test-only access to the advertiser (for teardown in tests).
    pub fn advertiser_for_test(&self) -> Arc<IrohAdvertiser> {
        self.advertiser.clone()
    }

    /// Test-only view of the addrs this device would dial for a share. The
    /// outbound half of revocation is invisible from the roster, so the
    /// revoked-peer tests assert on this set rather than on the sidecar file
    /// the set is loaded from.
    pub async fn known_peers_for_test(&self, shared_tree_id: &str) -> Vec<EndpointAddr> {
        self.known_peers
            .read()
            .await
            .get(shared_tree_id)
            .cloned()
            .unwrap_or_default()
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
/// A [`NestedShareRefusal`] out of a write closure keeps its type for the
/// caller; every other error stays as it was.
fn typed_share_error(e: anyhow::Error) -> Box<dyn std::error::Error + Send + Sync> {
    match e.downcast::<NestedShareRefusal>() {
        Ok(refusal) => Box::new(refusal),
        Err(e) => e.into(),
    }
}

/// What peers other than this doc's own have contributed to it.
fn remote_versions(doc: &LoroDoc) -> loro::VersionVector {
    let mut versions = doc.oplog_vv();
    versions.remove(&doc.peer_id());
    versions
}

/// Raises [`ConditionKind::NestedShareLoaded`] for each mount inside a shared
/// doc. Such a doc predates the nesting refusal or comes from an older peer;
/// it is disclosed, never repaired. Local writes cannot nest a share, so after
/// the first check only a change in remote versions triggers a scan.
struct NestedMountWatch {
    remote_versions: Option<loro::VersionVector>,
    disclosed: std::collections::HashSet<TreeID>,
}

impl NestedMountWatch {
    fn new() -> Self {
        Self {
            remote_versions: None,
            disclosed: Default::default(),
        }
    }

    fn check(&mut self, bus: &ConditionBus, shared_tree_id: &str, doc: &LoroDoc) {
        let versions = remote_versions(doc);
        if self.remote_versions.as_ref() == Some(&versions) {
            return;
        }
        self.remote_versions = Some(versions);
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let live = shared_tree::live_mounts(doc);
        for &node in live.difference(&self.disclosed) {
            let mount = read_stable_id(&tree, node)
                .map(|s| block_uri_from_bare(&s))
                .unwrap_or_else(|| format!("{node:?}"));
            warn!(shared_tree_id, %mount, "[share] a shared doc holds another share's mount");
            bus.emit(Condition {
                subject: shared_tree_id.to_string(),
                reason: ConditionKind::NestedShareLoaded { mount },
            });
        }
        self.disclosed = live;
    }
}

fn find_tree_id_by_stable_id(doc: &LoroDoc, stable_id: &EntityUri) -> Option<TreeID> {
    let needle = stable_id.id();
    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
    for node in tree.get_nodes(false) {
        if matches!(node.parent, TreeParentId::Deleted | TreeParentId::Unexist) {
            continue;
        }
        if read_stable_id(&tree, node.id).as_deref() == Some(needle) {
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
///
/// A mount whose `STABLE_ID` has not landed is an `Err`, not an empty id: the
/// caller projects the returned string as the mount's block URI, so an empty
/// one would write an unaddressable SQL row for a block that does have an
/// identity, just not yet a readable one.
/// The refusal of a second accept of a share this device already holds.
impl LoroShareBackend {
    fn claim_accept(&self, shared_tree_id: &str) -> Result<AcceptClaim> {
        let mut accepting = self.accepting.lock().expect("accept claims lock");
        if !accepting.insert(shared_tree_id.to_string()) {
            return Err(err(format!(
                "shared tree {shared_tree_id} is already being accepted on this device; a share \
                 has one placement per device, so this second accept is refused"
            )));
        }
        Ok(AcceptClaim {
            accepting: self.accepting.clone(),
            shared_tree_id: shared_tree_id.to_string(),
        })
    }
}

fn already_accepted(
    shared_tree_id: &str,
    handle: &str,
) -> Box<dyn std::error::Error + Send + Sync> {
    err(format!(
        "shared tree {shared_tree_id} is already accepted on this device as {handle}; a share has \
         one placement per device — move {handle} instead of accepting it again"
    ))
}

/// The live mount of `shared_tree_id` and the id the user knows the share by:
/// the page of a page share, the mount's own container row otherwise.
fn find_mount_by_shared_tree_id(
    doc: &LoroDoc,
    shared_tree_id: &str,
) -> anyhow::Result<Option<(TreeID, String)>> {
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
            if let Some(page) = info.placed_page() {
                return Ok(Some((node.id, page.to_string())));
            }
            let stable_id = read_stable_id(&tree, node.id)
                .map(|s| block_uri_from_bare(&s))
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "mount node {:?} for shared tree {shared_tree_id} has no STABLE_ID; \
                         cannot name the mount block it stands for",
                        node.id
                    )
                })?;
            return Ok(Some((node.id, stable_id)));
        }
    }
    Ok(None)
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
    // No commit: the only caller is inside an origin-armed write scope and
    // flushes through `WriteTxn::commit`, which labels the batch.
    Ok(node)
}

/// The kind of share the shared doc's root makes, read from the root's `Page`
/// tag. The tag is mutable, so this is read exactly once per share: when it is
/// made (and, for a share made before kinds were recorded, when rehydration
/// records it). Every later reader reads the recorded kind.
///
/// The shared subtree has exactly one root (extract_for_share reparents the
/// subtree root to the tree root); zero or many roots is a corrupt share.
fn share_kind_from_root_tag(shared_doc: &LoroDoc) -> Result<ShareKind> {
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
    Ok(if root.block.is_page() {
        ShareKind::Page {
            root: root.block.id.id().to_string(),
        }
    } else {
        ShareKind::Block
    })
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
        let (shared_arc, shared_root, mount_parent_uri, kind) = collab
            .with_write(WriteOrigin::ShareLifecycle, |txn| {
                let doc = txn.doc();

                let Some(tid) = find_tree_id_by_stable_id(doc, &id_uri) else {
                    if let Some(shared_tree_id) = self.shared_tree_holding(&id_uri) {
                        return Err(anyhow::Error::new(NestedShareRefusal::InsideShare {
                            id: id.to_string(),
                            shared_tree_id,
                        }));
                    }
                    anyhow::bail!("block {id} not found in Loro tree");
                };
                let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                if shared_tree::is_mount_node(&tree, tid) {
                    return Err(anyhow::Error::msg(format!(
                        "block {id} is already a mount node; sharing a mount is not supported"
                    )));
                }
                if let Some(mount) = shared_tree::first_mount_below(&tree, tid)? {
                    return Err(anyhow::Error::new(NestedShareRefusal::ContainsShare {
                        id: id.to_string(),
                        mount: read_stable_id(&tree, mount)
                            .map(|s| block_uri_from_bare(&s))
                            .unwrap_or_else(|| format!("{mount:?}")),
                    }));
                }
                // Amendment A: the mount is a Page (Inc 2), so it must sit under a
                // Page (or a root). Bubble the subtree's original parent to its
                // nearest page ancestor — a no-op in the common case (sharing a
                // page, or a block already under a page), so the mount stays in
                // place; only a block shared under a non-page bubbles up. Using this
                // `parent` for BOTH the SQL mount row and the Loro mount placement
                // keeps the two stores aligned.
                let parent = match parent_as_option(doc, tid) {
                    Some(ptid) => {
                        nearest_page_ancestor_tid(doc, ptid).map_err(anyhow::Error::msg)?
                    }
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
                                anyhow::Error::msg(format!(
                                    "shared subtree's parent node {parent_tid:?} has no STABLE_ID; \
                                 cannot resolve the mount's SQL parent"
                                ))
                            })?
                    }
                    None => EntityUri::no_parent().as_str().to_string(),
                };

                // --- Phase A: fork + extract (source unchanged) ---
                let extracted = shared_tree::extract_for_share(
                    doc,
                    tid,
                    parent,
                    shared_tree_id.clone(),
                    retention,
                )
                .map_err(|e| anyhow::Error::msg(format!("extract_for_share failed: {e:#}")))?;

                // Stable peer id BEFORE save so the persisted snapshot
                // already carries the right identity. Bump the generation so
                // this mint can never reuse a `(peer_id, counter)` from a
                // stale snapshot after a crash — see `share_peer_id`.
                let generation = self
                    .snapshot_store
                    .next_generation(&shared_tree_id)
                    .map_err(|e| anyhow::Error::msg(format!("bump peer-id generation: {e:#}")))?;
                let peer_id = stable_peer_id(&self.device_key, &shared_tree_id, generation);
                extracted
                    .shared_doc
                    .set_peer_id(peer_id)
                    .map_err(|e| anyhow::Error::msg(format!("set_peer_id on shared doc: {e:#}")))?;
                // The share's kind is decided here, once, and travels in the
                // shared doc to every recipient.
                let kind = share_kind_from_root_tag(&extracted.shared_doc)
                    .map_err(|e| anyhow::anyhow!("classify the shared root: {e}"))?;
                shared_tree::write_share_record(&extracted.shared_doc, &kind)?;

                // --- Persist shared snapshot BEFORE prune ---
                if let Err(e) = self
                    .snapshot_store
                    .save(&shared_tree_id, &extracted.shared_doc)
                {
                    self.degraded_bus.emit(Condition {
                        subject: shared_tree_id.clone(),
                        reason: ConditionKind::SnapshotSaveFailed(format!("{e:#}")),
                    });
                    // Source doc is still untouched — drop the extracted
                    // doc and bail out. No rollback needed.
                    return Err(anyhow::anyhow!(
                        "initial snapshot save failed; source tree unchanged: {e:#}"
                    ));
                }

                // --- Phase B: prune source + create mount node ---
                let shared_root = extracted.shared_root;
                let mount_tid = shared_tree::commit_share_prune(doc, &extracted)
                    .map_err(|e| anyhow::anyhow!("commit_share_prune failed: {e:#}"))?;
                set_stable_id(doc, mount_tid, &mount_stable_id)
                    .map_err(|e| anyhow::anyhow!("set mount stable_id: {e:#}"))?;
                shared_tree::record_mount(
                    &doc.get_tree(crate::loro_backend::TREE_NAME),
                    mount_tid,
                    &kind,
                    MountRole::Owner,
                )?;
                // Not `doc.commit()`: `commit_share_prune` already consumed the
                // scope's armed origin, so a bare commit here would land
                // unlabelled and a text-undo manager would offer to take the
                // share back.
                txn.commit();

                Ok((
                    Arc::new(extracted.shared_doc),
                    shared_root,
                    mount_parent_uri,
                    kind,
                ))
            })
            .map_err(typed_share_error)?;

        // Flush the global doc so the mount node survives in lockstep
        // with the shared snapshot. Failure here leaves consistent
        // memory but inconsistent disk; emit a degraded signal so the
        // controller's next save cycle reconciles things. Return Err
        // because the caller didn't get a ticket — the op failed.
        if let Err(e) = self.store.read().await.save_all().await {
            self.degraded_bus.emit(Condition {
                subject: shared_tree_id.clone(),
                reason: ConditionKind::SnapshotSaveFailed(format!(
                    "global doc save_all failed after fork-prune: {e:#}"
                )),
            });
            return Err(err(format!("global doc save_all failed: {e:#}")));
        }

        // Sharer-side SQL projection. `accept_shared_subtree` projects the
        // shared subtree so the UI (which reads SQL, not Loro) renders
        // shared content; `share_subtree` must too, or the sharing peer loses
        // the subtree from the UI until the next restart re-hydrates it.
        //
        // Ordering matters. The prune commit above removed every descendant
        // block from the GLOBAL tree, so the global Loro→SQL projection's next
        // diff emits a DELETE for each. That is a ONE-TIME event: the global
        // projection diffs the global doc against an advancing base, so once
        // the pruned snapshot becomes the base it never re-deletes those ids.
        // If we re-created the descendant rows BEFORE that delete pass ran, the
        // delete would wipe them again. We defeat it deterministically:
        //   1. register the shared doc, so every reader that follows the mount finds it
        //      and a full reseed of the global projection counts the share's rows as
        //      the share's;
        //   2. a BLOCK share projects its container row (an UPSERT; carries
        //      `share-role`) — a page share has none, its page keeps its own row;
        //   3. flush the global projection — this acquires its project lock
        //      (serializing with the background loop), drains the pending prune-delete,
        //      and advances its base, so no later pass can re-delete the descendants;
        //   4. re-project the subtree as the LAST write.
        // Without the DI-wired projection (tests) there is no global loop to
        // race, so the flush is simply skipped.
        self.manager
            .register_arc(shared_tree_id.clone(), shared_arc.clone());
        if kind == ShareKind::Block {
            self.project_container_to_sql(&mount_stable_id, &mount_parent_uri, &shared_tree_id)
                .await?;
        }
        if let Some(projection) = self.downstream_projection.as_ref() {
            let pass = projection.flush().await.map_err(|e| {
                err(format!(
                    "flush global projection before re-projecting shared descendants: {e:#}"
                ))
            })?;
            // The prune-delete this flush publishes MUST land before the shared
            // descendants are re-projected, or the delete races and re-removes
            // the just-re-created rows. A withheld op means it did not land.
            if pass.withheld() > 0 {
                return Err(err(format!(
                    "the global projection withheld {} FK-ungrounded op(s), so the prune-delete \
                     did not reach SQL before re-projecting shared descendants",
                    pass.withheld()
                )));
            }
        }
        self.project_descendants_to_sql(&shared_arc, &shared_tree_id)
            .await?;

        // Mint the share's access capability + enrollment deadline BEFORE
        // advertising: the capability is what the acceptor roster is keyed on,
        // and a share must never reach the endpoint stage without one. The
        // capability — not the leaky `shared_tree_id` — is the real secret a
        // recipient proves possession of (see `share_enrollment`).
        let capability = CapabilitySecret::generate();
        let expires_at =
            ExpiryTime(chrono::Utc::now().timestamp() + DEFAULT_ENROLLMENT_WINDOW_SECS);
        let roster = self
            .install_roster(&shared_tree_id, &capability, expires_at)
            .await?;

        let addr = self
            .start_advertising_stable(
                &shared_tree_id,
                shared_arc.clone(),
                ShareAdmission::Enrolled {
                    roster,
                    // A share is a shared workspace: an enrolled peer authors
                    // into it (D72.a). What bounds the blast radius is who may
                    // enroll, not what membership confers.
                    capabilities: Capabilities::read_write(),
                },
            )
            .await
            .map_err(|e| err(format!("start advertiser: {e:#}")))?;

        self.attach_save_worker(shared_tree_id.clone(), shared_arc.clone())
            .await;
        self.attach_sync_worker(shared_tree_id.clone(), shared_arc.clone())
            .await;
        self.attach_projection_worker(shared_tree_id.clone(), shared_arc)
            .await?;

        let alpn = format!("{ALPN_PREFIX}/{shared_tree_id}");
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
        // The ticket's capability is the share's access secret in both
        // directions: we prove it to the author when we dial, and our own
        // roster is keyed on it so the author (and only a peer holding it) can
        // dial us back. Possession of the ticket is therefore still what grants
        // access — but it is now the only thing that does, and it is bounded by
        // the enrollment window, the peer cap, and revocation.
        let t = Ticket::decode(&ticket).map_err(|e| err(format!("decode ticket: {e:#}")))?;

        // One placement per share on this device. A second accept names a
        // second place for the SAME page, and a page has one parent; refuse it
        // before anything touches the network, and name the mount the user can
        // move instead. The claim covers an accept still in flight, the mount
        // one that finished.
        let _claim = self.claim_accept(&t.shared_tree_id)?;
        if let Some((_, existing)) = self
            .global_doc()
            .await?
            .with_read(|doc| find_mount_by_shared_tree_id(doc, &t.shared_tree_id))?
        {
            return Err(already_accepted(&t.shared_tree_id, &existing));
        }

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
        // Our copy of the share is gated on the SAME capability the ticket
        // carried, so the author can dial us back and a stranger who learned
        // the `shared_tree_id` cannot. The expiry we adopt is the ticket's:
        // one window governs enrollment into this share on both ends.
        let roster = self
            .install_roster(&shared_tree_id, &t.capability, t.expires_at)
            .await?;
        match self
            .start_advertising_stable(
                &shared_tree_id,
                shared_arc.clone(),
                ShareAdmission::Enrolled {
                    roster,
                    capabilities: Capabilities::read_write(),
                },
            )
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
        // The ticket author owns the subtree we just accepted, so it authors
        // into our copy. We prove the ticket's capability to it before any
        // bytes move; a forged ticket gets no further than this dial.
        let initiate = sync_doc_initiate_enrolled(
            &client_ep,
            &shared_arc,
            &alpn_bytes,
            t.addr.clone(),
            &t.capability,
            &shared_tree_id,
            Capabilities::read_write(),
        );
        let conn = timeout(CONNECT_TIMEOUT, initiate)
            .await
            .map_err(|_| err("initial sync timed out"))?
            .map_err(|e| err(format!("initial sync failed: {e:#}")))?;

        // Pin the author we just dialed. QUIC authenticated it as the node key
        // the ticket named and it accepted our capability proof, so its later
        // inbound dials need no fresh enrollment — which matters because those
        // can fall outside the ticket's window.
        // Read the roster back from the advertiser rather than using the one
        // built above: on the "already advertising" branch the LIVE roster is
        // an earlier one, and pinning into a detached copy would pin nothing.
        let live_roster = self
            .advertiser
            .roster_for(&shared_tree_id)
            .await
            .ok_or_else(|| {
                err(format!(
                    "share {shared_tree_id} is advertised without a roster right after an enrolled \
                     accept"
                ))
            })?;
        {
            let mut guard = live_roster.lock().await;
            guard
                .pin_dialed(peer_fingerprint(&conn))
                .map_err(|e| err(format!("pin the share author into our roster: {e}")))?;
        }
        self.persist_roster(&shared_tree_id).await?;
        let _conn = conn;

        // Persist the shared snapshot BEFORE creating a mount node in
        // the global tree. If save fails, no mount node has been
        // created — drop the doc and return Err.
        if let Err(e) = self.snapshot_store.save(&shared_tree_id, &shared_arc) {
            self.degraded_bus.emit(Condition {
                subject: shared_tree_id.clone(),
                reason: ConditionKind::SnapshotSaveFailed(format!("{e:#}")),
            });
            return Err(err(format!(
                "initial snapshot save failed after sync; global tree unchanged: {e:#}"
            )));
        }

        // The sharer decided the share's kind; this device records the same.
        let kind = shared_tree::read_share_record(&shared_arc)
            .map_err(|e| err(format!("read the share record of {shared_tree_id}: {e:#}")))?
            .ok_or_else(|| {
                err(format!(
                    "shared tree {shared_tree_id} carries no share record, so this device cannot \
                     tell whether it shares a page or a block, and does not place it. The share \
                     was made before share kinds were recorded; a share gets its record only when \
                     it is made, so updating either device does not add one. Ask the owner to \
                     unshare it and share it again, then accept the new ticket."
                ))
            })?;

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

        let collab = self.global_doc().await?;
        let no_parent = EntityUri::no_parent().as_str().to_string();
        // Set when the mount is parented under the "Shared with me" recipient
        // root (H7) — drives the post-lock SQL projection of that root row.
        let mut attached_to_shared_with_me = false;
        let (mount_stable_id, mount_parent_uri) = collab
            .with_write(WriteOrigin::ShareLifecycle, |txn| {
                let doc = txn.doc();
                let new_id = format!("block:{}", Uuid::new_v4());
                let Some(parent_tid) = find_tree_id_by_stable_id(doc, &parent_uri) else {
                    if let Some(shared_tree_id) = self.shared_tree_holding(&parent_uri) {
                        return Err(anyhow::Error::new(NestedShareRefusal::InsideShare {
                            id: parent_uri.to_string(),
                            shared_tree_id,
                        }));
                    }
                    anyhow::bail!("parent block {parent_id} not found");
                };
                // Amendment A: the mount places a Page, so bubble the accept target
                // to its nearest page ancestor — a no-op when the user targeted
                // a page (the common case).
                //
                // H7 (ADR 0028): when the target has NO page ancestor, the mount
                // would orphan at `no_parent` — present in SQL but invisible in
                // the UI (dogfood N3, 2026-07-20). Attach it under the dedicated
                // "Shared with me" recipient root instead, so accepted shares are
                // always reachable and rendered.
                let page_parent_tid =
                    match nearest_page_ancestor_tid(doc, parent_tid).map_err(anyhow::Error::msg)? {
                        Some(p) => Some(p),
                        None => {
                            attached_to_shared_with_me = true;
                            Some(ensure_shared_with_me_root_node(doc).map_err(anyhow::Error::msg)?)
                        }
                    };

                let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                let mount = shared_tree::create_mount_node(
                    &tree,
                    page_parent_tid,
                    &shared_tree_id,
                    shared_root,
                )
                .map_err(|e| anyhow::anyhow!("create mount node: {e:#}"))?;
                set_stable_id(doc, mount, &new_id)
                    .map_err(|e| anyhow::anyhow!("set mount stable_id: {e:#}"))?;
                shared_tree::record_mount(&tree, mount, &kind, MountRole::Recipient)?;
                // Not `doc.commit()`: `ensure_shared_with_me_root_node` may
                // already have consumed the scope's armed origin.
                txn.commit();
                let mount_parent_uri = match page_parent_tid {
                    Some(ptid) => read_stable_id(&tree, ptid)
                        .map(|s| block_uri_from_bare(&s))
                        .ok_or_else(|| {
                            anyhow::anyhow!("page ancestor {ptid:?} has no STABLE_ID")
                        })?,
                    None => no_parent.clone(),
                };
                Ok((new_id, mount_parent_uri))
            })
            .map_err(typed_share_error)?;

        // Flush the global doc so the mount node is durable.
        if let Err(e) = self.store.read().await.save_all().await {
            self.degraded_bus.emit(Condition {
                subject: shared_tree_id.clone(),
                reason: ConditionKind::SnapshotSaveFailed(format!(
                    "global doc save_all failed after accept: {e:#}"
                )),
            });
            return Err(err(format!("global doc save_all failed: {e:#}")));
        }

        // H7: if the mount was parented under the "Shared with me" recipient
        // root, project that root row FIRST so the mount's SQL parent resolves
        // to a rendered page (the UI reads SQL). Idempotent (the create is an UPSERT).
        if attached_to_shared_with_me {
            self.project_shared_with_me_root_to_sql().await?;
        }

        // Register the shared doc BEFORE projecting: every reader that follows
        // the mount (the write-back walking the page's children first) must
        // find it the moment the rows land. A block share then projects its
        // synthetic container page; a page share's page is its own row.
        self.manager
            .register_arc(shared_tree_id.clone(), shared_arc.clone());
        if kind == ShareKind::Block {
            self.project_container_to_sql(&mount_stable_id, &mount_parent_uri, &shared_tree_id)
                .await?;
        }
        self.project_descendants_to_sql(&shared_arc, &shared_tree_id)
            .await?;

        // Advertiser was started before the initial sync (see top of
        // this function) so the dialer endpoint is long-lived and the
        // remote peer's accept-loop callback records an addr that's
        // still valid after the sync completes.

        self.attach_sync_worker(shared_tree_id.clone(), shared_arc.clone())
            .await;
        self.attach_projection_worker(shared_tree_id.clone(), shared_arc.clone())
            .await?;
        self.attach_save_worker(shared_tree_id.clone(), shared_arc)
            .await;

        // The handle is what the user knows the share by, and what `unshare`
        // takes: the page of a page share, the container row otherwise.
        let handle = match &kind {
            ShareKind::Page { root } => EntityUri::block(root).to_string(),
            ShareKind::Block => mount_stable_id.clone(),
        };
        let response = serde_json::json!({
            "mount_block_id": mount_stable_id,
            "shared_tree_id": shared_tree_id,
            "handle": handle,
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
        let known: std::collections::HashSet<String> = collab.with_read(|doc| {
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            Ok(tree
                .get_nodes(false)
                .into_iter()
                .filter(|n| !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist))
                .filter(|n| shared_tree::is_mount_node(&tree, n.id))
                .filter_map(|n| shared_tree::read_mount_info(&tree, n.id))
                .map(|m| m.shared_tree_id)
                .collect())
        })?;

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

    async fn unshare(&self, id: &str) -> Result<OperationResult> {
        let uri = EntityUri::parse(id)
            .map_err(|e| err(format!("invalid share handle URI {id:?}: {e:#}")))?;
        if !uri.is_block() {
            return Err(err(format!(
                "unshare expects a `block:` URI, got scheme {:?} (full URI: {id:?})",
                uri.scheme()
            )));
        }
        // (1) Resolve the handle to its mount: a page share is known by its
        // page, any other share by its mount's own container row.
        let collab = self.global_doc().await?;
        let (mount_tid, info) = collab.with_read(|doc| {
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            if let Some(tid) = find_tree_id_by_stable_id(doc, &uri) {
                let info = shared_tree::read_mount_info(&tree, tid).ok_or_else(|| {
                    anyhow::Error::msg(format!(
                        "block {id} is neither a shared page nor a mount; unshare applies to \
                         shares only"
                    ))
                })?;
                if let Some(page) = info.placed_page() {
                    anyhow::bail!(
                        "block {id} is the internal placement record of the shared page {page}; \
                         unshare {page} instead"
                    );
                }
                return Ok((tid, info));
            }
            tree.get_nodes(false)
                .into_iter()
                .filter(|n| !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist))
                .find_map(|n| {
                    shared_tree::read_mount_info(&tree, n.id)
                        .filter(|info| info.placed_page().as_ref() == Some(&uri))
                        .map(|info| (n.id, info))
                })
                .ok_or_else(|| {
                    anyhow::Error::msg(format!(
                        "block {id} is neither a shared page this device holds nor a mount"
                    ))
                })
        })?;
        self.teardown_share(mount_tid, info).await?;
        let response = serde_json::json!({ "unshared": id });
        Ok(
            OperationResult::irreversible(vec![])
                .with_response(Value::String(response.to_string())),
        )
    }
}

impl LoroShareBackend {
    /// Leave `shared_tree_id` on this device — what a recipient's delete of a
    /// placed page does.
    pub async fn leave_share(&self, shared_tree_id: &str) -> Result<()> {
        let collab = self.global_doc().await?;
        let (mount_tid, info) = collab.with_read(|doc| {
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            let mount = shared_tree::find_mount_node(&tree, shared_tree_id).ok_or_else(|| {
                anyhow::anyhow!("shared tree {shared_tree_id} has no mount on this device")
            })?;
            let info = shared_tree::read_mount_info(&tree, mount)
                .ok_or_else(|| anyhow::anyhow!("mount {mount:?} carries no mount metadata"))?;
            Ok((mount, info))
        })?;
        self.teardown_share(mount_tid, info).await
    }

    /// Tear a share down on this device, in a resurrection-safe order. A
    /// recipient leaving a page share is disclosed: the page vanishes here and
    /// nowhere else.
    async fn teardown_share(&self, mount_tid: TreeID, info: shared_tree::MountInfo) -> Result<()> {
        let shared_tree_id = info.shared_tree_id.clone();
        let collab = self.global_doc().await?;
        let mount_row = collab.with_read(|doc| {
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            Ok(read_stable_id(&tree, mount_tid).map(|s| block_uri_from_bare(&s)))
        })?;
        let left_page = match (info.role, info.placed_page()) {
            (Some(MountRole::Recipient), Some(page)) => {
                let title = self
                    .manager
                    .get_doc(&shared_tree_id)
                    .and_then(|doc| {
                        crate::loro_backend::snapshot_blocks_from_doc(&doc)
                            .remove(page.as_str())
                            .map(|s| s.block.content)
                    })
                    .unwrap_or_else(|| page.to_string());
                Some((page, title))
            }
            _ => None,
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

        // (5) Delete the mount node from the global tree, then flush to disk.
        // The share's rows are the share projection's alone — the global
        // projection skips mounts and never reads the shared doc — so they are
        // deleted here, explicitly.
        collab.with_write(WriteOrigin::ShareLifecycle, |txn| {
            let doc = txn.doc();
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            // Name the mount's root containers BEFORE the delete: the delete
            // cascades and a gone node no longer names its roots. Without the
            // purge they outlive it, holding their content in the global doc's
            // state and so in every export of it.
            let mount_roots = crate::deleted_container_purge::subtree_roots(&tree, mount_tid)
                .map_err(|e| anyhow::anyhow!("collect mount node roots to purge: {e:#}"))?;
            tree.delete(mount_tid)
                .map_err(|e| anyhow::anyhow!("delete mount node {mount_tid:?}: {e:#}"))?;
            crate::deleted_container_purge::purge_roots(doc, &mount_roots)
                .map_err(|e| anyhow::anyhow!("purge mount node containers: {e:#}"))?;
            Ok(())
        })?;
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
            if let Some(mount_row) = mount_row.filter(|_| info.placed_page().is_none()) {
                let mut params = StorageEntity::new();
                params.insert("id".into(), Value::String(mount_row.clone()));
                sql_ops
                    .execute_operation(&entity, "delete", params)
                    .await
                    .map_err(|e| {
                        err(format!(
                            "unshare: delete container row {mount_row} from SQL: {e}"
                        ))
                    })?;
            }
        }

        // (6) Delete the on-disk snapshot — now safe, all workers are gone.
        // This also removes the roster sidecar, so no restart can rebuild an
        // acceptor roster for a share that no longer exists here.
        self.snapshot_store
            .delete_snapshot(&shared_tree_id)
            .map_err(|e| err(format!("delete_snapshot({shared_tree_id}): {e:#}")))?;

        // (7) Revoke the share as a whole: drop its capability secret. Every
        // ticket ever issued for it is now inert — this device can no longer
        // build the roster a holder would prove itself against, and cannot
        // prove itself to anyone else either.
        self.credentials
            .forget_capability(&shared_tree_id)
            .map_err(|e| err(format!("revoke the capability for {shared_tree_id}: {e:#}")))?;

        if let Some((page, title)) = left_page {
            self.degraded_bus.emit(Condition {
                subject: page.to_string(),
                reason: ConditionKind::LeftSharedPage { title },
            });
        }
        Ok(())
    }
}

#[async_trait]
impl shared_tree::ShareExit for LoroShareBackend {
    async fn leave(&self, shared_tree_id: &str) -> anyhow::Result<()> {
        self.leave_share(shared_tree_id)
            .await
            .map_err(|e| anyhow::anyhow!("leave shared tree {shared_tree_id}: {e}"))
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

        // A share whose kind cannot be told is not loaded at all: nothing
        // could place its root, and an unregistered share's rows are the
        // global reseed's to retract.
        let kind = match &info.kind {
            shared_tree::KindRecord::Recorded(kind) => Ok(kind.clone()),
            shared_tree::KindRecord::Unrecorded => {
                backend
                    .record_legacy_share_kind(&shared_tree_id, &doc)
                    .await
            }
            shared_tree::KindRecord::Corrupt(why) => Err(err(format!(
                "the mount records an unreadable share kind: {why}"
            ))),
        };
        let kind = match kind {
            Ok(kind) => kind,
            Err(e) => {
                warn!(
                    shared_tree_id = %shared_tree_id,
                    error = %e,
                    "[share] the share's kind is unknown; the share is not loaded"
                );
                backend.degraded_bus.emit(Condition {
                    subject: shared_tree_id.clone(),
                    reason: ConditionKind::RehydrationFailed(format!(
                        "share kind unknown, share not loaded: {e:#}"
                    )),
                });
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
                backend.degraded_bus.emit(Condition {
                    subject: shared_tree_id.clone(),
                    reason: ConditionKind::RehydrationFailed(format!("next_generation: {e:#}")),
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
            backend.degraded_bus.emit(Condition {
                subject: shared_tree_id.clone(),
                reason: ConditionKind::RehydrationFailed(format!("set_peer_id: {e:#}")),
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
                backend.degraded_bus.emit(Condition {
                    subject: shared_tree_id.clone(),
                    reason: ConditionKind::RehydrationFailed(format!("load_peers: {e:#}")),
                });
            }
        }

        // Rebuild the acceptor roster from the keychain capability + the
        // owner-signed sidecar. A share whose roster cannot be rebuilt is NOT
        // advertised: serving it un-gated would hand every peer that knows the
        // (leaky) `shared_tree_id` read+write, which is exactly the hole this
        // path closes. The share stays registered and syncable outbound, and
        // the failure is disclosed rather than silently downgraded.
        let roster = match backend.rehydrate_roster(&shared_tree_id).await {
            Ok(roster) => Some(roster),
            Err(e) => {
                warn!(
                    shared_tree_id = %shared_tree_id,
                    error = %e,
                    "[share] roster could not be rebuilt; refusing to advertise this share"
                );
                backend.degraded_bus.emit(Condition {
                    subject: shared_tree_id.clone(),
                    reason: ConditionKind::RehydrationFailed(format!(
                        "roster unavailable, share not advertised: {e:#}"
                    )),
                });
                None
            }
        };

        // Start advertising. "Already advertising" is success (e.g.,
        // two rehydration paths got wired up). Other errors are
        // degraded-mode but non-fatal — the share is still in the
        // registry and can be pulled from.
        if let Some(roster) = roster {
            match backend
                .start_advertising_stable(
                    &shared_tree_id,
                    arc.clone(),
                    ShareAdmission::Enrolled {
                        roster,
                        capabilities: Capabilities::read_write(),
                    },
                )
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
                    backend.degraded_bus.emit(Condition {
                        subject: shared_tree_id.clone(),
                        reason: ConditionKind::RehydrationFailed(format!("advertiser: {e:#}")),
                    });
                    // Intentionally continue — the share is usable for
                    // pulls even without advertising.
                }
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

        // Re-project the share into SQL. The `create` op is an UPSERT, so
        // this is safe across restarts and repairs rows lost while the Loro
        // snapshot survived. A block share also re-projects its container row,
        // which needs the mount's own stable id and its parent's; either
        // missing skips that row with a warn!.
        let container = (kind == ShareKind::Block).then_some((
            record.mount_stable_id.as_deref(),
            record.parent_stable_id.as_deref(),
        ));
        match container {
            Some((Some(mount_bare), Some(parent_bare))) => {
                if let Err(e) = backend
                    .project_container_to_sql(
                        &block_uri_from_bare(mount_bare),
                        &block_uri_from_bare(parent_bare),
                        &shared_tree_id,
                    )
                    .await
                {
                    warn!(
                        shared_tree_id = %shared_tree_id,
                        error = %e,
                        "[share] project_container_to_sql during rehydrate failed"
                    );
                }
            }
            Some(_) => warn!(
                shared_tree_id = %shared_tree_id,
                mount_stable_id = ?record.mount_stable_id,
                parent_stable_id = ?record.parent_stable_id,
                "[share] skipping the container row projection — missing stable id(s)"
            ),
            None => {}
        }
        if let Err(e) = backend
            .project_descendants_to_sql(&arc, &shared_tree_id)
            .await
        {
            warn!(
                shared_tree_id = %shared_tree_id,
                error = %e,
                "[share] project_descendants_to_sql during rehydrate failed"
            );
        }
        if let Err(e) = backend
            .attach_projection_worker(shared_tree_id.clone(), arc.clone())
            .await
        {
            warn!(
                shared_tree_id = %shared_tree_id,
                error = %e,
                "[share] attach_projection_worker during rehydrate failed"
            );
        }

        rehydrated += 1;
    }

    Ok(rehydrated)
}

impl LoroShareBackend {
    /// Record the kind of a share mounted before kinds were recorded: the
    /// sharer's record in the shared doc when there is one, else the root's
    /// `Page` tag, read this once.
    async fn record_legacy_share_kind(
        &self,
        shared_tree_id: &str,
        shared_doc: &LoroDoc,
    ) -> Result<ShareKind> {
        let kind = match shared_tree::read_share_record(shared_doc)
            .map_err(|e| err(format!("read the share record: {e:#}")))?
        {
            Some(kind) => kind,
            None => share_kind_from_root_tag(shared_doc)?,
        };
        self.global_doc()
            .await?
            .with_write(WriteOrigin::ShareLifecycle, |txn| {
                let tree = txn.doc().get_tree(crate::loro_backend::TREE_NAME);
                let mount =
                    shared_tree::find_mount_node(&tree, shared_tree_id).ok_or_else(|| {
                        anyhow::anyhow!("shared tree {shared_tree_id} lost its mount mid-rehydrate")
                    })?;
                shared_tree::record_share_kind(&tree, mount, &kind)?;
                txn.commit();
                Ok(())
            })?;
        tracing::info!(
            shared_tree_id = %shared_tree_id,
            kind = ?kind,
            "[share] recorded the kind of a share mounted before kinds were recorded"
        );
        Ok(kind)
    }
}

/// Internal record bundling a `MountInfo` with the stable ids needed to
/// project the mount row into SQL. Lives in this module rather than
/// `shared_tree.rs` because the consumer (rehydrate) is the only caller.
struct MountRehydrationRecord {
    info: shared_tree::MountInfo,
    mount_stable_id: Option<String>,
    parent_stable_id: Option<String>,
}

/// The share backend's entry into the settled-read policy: `Some` only for a
/// node whose `STABLE_ID` has landed. The returned id is whatever the meta
/// holds — callers should not assume presence or absence of a `block:` scheme
/// prefix (see [`block_uri_from_bare`] for the normalization).
///
/// Every caller that cannot proceed without an id turns the `None` into an
/// `Err` naming the node; the ones that may skip do so knowingly.
fn read_stable_id(tree: &loro::LoroTree, tid: TreeID) -> Option<String> {
    match classify(tree, tid) {
        LiveNode::Settled(sid) => Some(sid),
        // No id exists to answer with: an in-flight create has not landed its
        // STABLE_ID insert yet, or a concurrent commit removed the node
        // between enumeration and this read.
        LiveNode::HalfBorn | LiveNode::MetaUnreadable => None,
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

    /// A device's keychain, as a value a restart can be handed again. The
    /// production keychain outlives the process; an in-memory double only
    /// does so if the test keeps the same instance across both backends.
    fn test_keychain() -> Arc<holon_secrets::InMemoryKeychainStore> {
        Arc::new(holon_secrets::InMemoryKeychainStore::new())
    }

    fn test_credentials(
        keychain: Arc<holon_secrets::InMemoryKeychainStore>,
    ) -> Arc<ShareCredentials> {
        let owner_keychain = keychain.clone();
        Arc::new(ShareCredentials::with_stores(
            keychain,
            crate::owner_identity::OwnerCustody::with_keychain(owner_keychain, "test-owner"),
        ))
    }

    /// Rebuild a backend over an existing storage dir and keychain — the
    /// restart the rehydrate path is written for.
    fn make_backend_at(
        dir_path: &std::path::Path,
        credentials: Arc<ShareCredentials>,
    ) -> Arc<LoroShareBackend> {
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(dir_path.to_path_buf())));
        let bus = Arc::new(ConditionBus::new());
        let snapshot_store = Arc::new(SharedSnapshotStore::new(
            dir_path.to_path_buf(),
            bus.clone(),
        ));
        let manager = Arc::new(SharedTreeSyncManager::new());
        // Use the persistent key-from-disk path so a `drop+re-make`
        // simulates a real process restart (same device identity).
        let key = crate::device_key_store::load_or_create_device_key(dir_path).unwrap();
        let advertiser = Arc::new(IrohAdvertiser::new_with_key(key.clone()));
        LoroShareBackend::new(
            store,
            snapshot_store,
            manager,
            advertiser,
            bus,
            key,
            credentials,
        )
    }

    fn make_backend() -> (Arc<LoroShareBackend>, TempDir) {
        let (backend, _keychain, dir) = make_backend_with_keychain();
        (backend, dir)
    }

    /// Rehydrate a freshly-rebuilt backend's shares — the restart every
    /// rehydrate test performs.
    ///
    /// The raw-doc escape lives HERE, once, rather than at each call site:
    /// `rehydrate_shared_trees` is async and so cannot run inside
    /// `LoroDocument::with_read`'s synchronous closure. This mirrors the one
    /// production caller, `holon-loro-wiring`'s `loro_module.rs`, which carries
    /// the same escape for the same reason.
    async fn rehydrate_over(backend: &Arc<LoroShareBackend>) -> usize {
        let collab = backend.test_global_doc().await;
        // ALLOW(loro_doc_escape): async consumer, single-threaded test, no
        // concurrent writer.
        let doc_arc = collab.doc();
        rehydrate_shared_trees(backend, &doc_arc)
            .await
            .expect("rehydrate_shared_trees")
    }

    fn make_backend_with_keychain() -> (
        Arc<LoroShareBackend>,
        Arc<holon_secrets::InMemoryKeychainStore>,
        TempDir,
    ) {
        let dir = TempDir::new().unwrap();
        let keychain = test_keychain();
        let backend = make_backend_at(dir.path(), test_credentials(keychain.clone()));
        (backend, keychain, dir)
    }

    /// `share_subtree` mints its roster with `DEFAULT_ENROLLMENT_WINDOW_SECS`,
    /// which bounds who may still ENROLL. A recipient who accepted inside that
    /// window keeps syncing afterwards; unsharing is what ends the share.
    #[test]
    fn an_accepted_subtree_share_still_authorises_its_enrolled_peer_at_day_31() {
        use crate::share_enrollment::Challenge;
        use crate::share_enrollment::EnrollmentProofMsg;
        use crate::share_enrollment::ExpiryTime;
        use crate::share_enrollment::PeerFingerprint;
        use crate::share_enrollment::ShareRoster;

        let minted_at = 1_700_000_000;
        let capability = crate::share_enrollment::CapabilitySecret::generate();
        let mut roster = ShareRoster::new(
            "tree-shared",
            capability.clone(),
            ExpiryTime(minted_at + DEFAULT_ENROLLMENT_WINDOW_SECS),
            4,
        );
        let challenge = Challenge::generate();
        let proof = EnrollmentProofMsg::build(&capability, &challenge, "tree-shared");
        let recipient = PeerFingerprint::from_bytes([7u8; 32]);
        roster
            .authorize(
                minted_at + 60,
                &challenge,
                &proof.capability_id,
                &proof.proof,
                recipient,
            )
            .expect("the recipient accepts the share inside the enrollment window");

        let day_31 = minted_at + 31 * 24 * 60 * 60;
        let reconnect = Challenge::generate();
        let authorized = roster
            .authorize(
                day_31,
                &reconnect,
                &proof.capability_id,
                &proof.proof,
                recipient,
            )
            .expect("an accepted share still authorises its recipient at day 31");
        assert!(!authorized.newly_enrolled());
    }

    /// A mount node found by its shared-tree id must come back with the block
    /// URI it stands for. If its own `STABLE_ID` has not landed the lookup
    /// errs: the caller writes the returned string as the mount's SQL block
    /// id, and an empty one is an unaddressable row for a block that does have
    /// an identity — just not a readable one yet.
    #[test]
    fn a_mount_without_a_stable_id_errs_instead_of_naming_an_empty_block() {
        use crate::loro_backend::TREE_NAME;

        let doc = LoroDoc::new();
        let tree = doc.get_tree(TREE_NAME);
        let shared_root = tree.create(None).unwrap();
        let mount = shared_tree::create_mount_node(&tree, None, "tree-77", shared_root).unwrap();
        doc.commit();

        let err = find_mount_by_shared_tree_id(&doc, "tree-77")
            .expect_err("a mount with no STABLE_ID must not resolve to an empty block id");
        let msg = err.to_string();
        assert!(
            msg.contains("has no STABLE_ID"),
            "the error must name the missing id: {msg}"
        );

        // The settled companion: once the id lands, the same lookup answers.
        set_stable_id(&doc, mount, "block:99999999-9999-9999-9999-999999999999").unwrap();
        doc.commit();
        let (tid, uri) = find_mount_by_shared_tree_id(&doc, "tree-77")
            .unwrap()
            .expect("the mount is still there");
        assert_eq!(tid, mount);
        assert_eq!(uri, "block:99999999-9999-9999-9999-999999999999");
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

        let (ops, settled) = share_diff_ops(
            &doc,
            &before,
            &RootPlacement::Container {
                mount: EntityUri::block("mount"),
            },
            "test-tree",
        );
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

        let (ops, settled) = share_diff_ops(
            &doc,
            &before,
            &RootPlacement::Container {
                mount: EntityUri::block("mount"),
            },
            "test-tree",
        );
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
            msg.contains("not found") || msg.contains("get_doc(Global)"),
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
        // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
        // exists to observe.
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
        // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
        // exists to observe.
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
        // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
        // exists to observe.
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

    /// Sharing and accepting must never look like typing.
    ///
    /// Both seams commit the global document more than once inside one write
    /// scope — a helper flushes first, then the mount node and its stable id
    /// are flushed after it. Loro arms an origin for one commit only, so
    /// without [`WriteTxn::commit`] re-arming, everything after the helper
    /// lands under the empty origin and a text-undo manager that excludes the
    /// system prefix would offer to take a share back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn share_and_accept_commit_only_under_system_origins() {
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

        let watch = |collab: &Arc<crate::LoroDocument>| {
            let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
            let sink = seen.clone();
            // ALLOW(loro_doc_escape): subscription registration, a blessed use.
            let sub = collab.doc().subscribe_root(Arc::new(move |event| {
                sink.lock().unwrap().push(event.origin.to_string());
            }));
            (seen, sub)
        };
        let collab_a = backend_a.global_doc().await.unwrap();
        let collab_b = backend_b.global_doc().await.unwrap();
        let (seen_a, _sub_a) = watch(&collab_a);
        let (seen_b, _sub_b) = watch(&collab_b);

        let share_response = backend_a
            .share_subtree("block:shared-parent", "none".into())
            .await
            .unwrap();
        let ticket_json: serde_json::Value = match share_response.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response type: {other:?}"),
        };
        let ticket = ticket_json["ticket"].as_str().unwrap().to_string();
        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();

        for (leg, seen) in [("share", &seen_a), ("accept", &seen_b)] {
            let origins = seen.lock().unwrap().clone();
            assert!(
                !origins.is_empty(),
                "the {leg} leg committed nothing to the global doc, so this proves nothing"
            );
            assert!(
                origins
                    .iter()
                    .all(|o| o.starts_with(WriteOrigin::SYSTEM_PREFIX)),
                "the {leg} leg committed under origin(s) a text-undo manager cannot exclude: \
                 {origins:?}"
            );
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
            // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer exists
            // to observe.
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
            // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer exists
            // to observe.
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
            // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer exists
            // to observe.
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

    /// The mount node's block URI in A's global tree after a share.
    async fn mount_uri_of(backend_a: &LoroShareBackend) -> String {
        let a_global = backend_a.global_doc().await.unwrap();
        let mount_tid = a_global
            .with_read(|doc| {
                let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                Ok(tree
                    .get_nodes(false)
                    .iter()
                    .find(|n| {
                        !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist)
                            && shared_tree::is_mount_node(&tree, n.id)
                    })
                    .map(|n| n.id))
            })
            .unwrap()
            .expect("A's global tree must hold a mount node after share");
        EntityUri::block_from_tree_id(mount_tid.peer, mount_tid.counter).to_string()
    }

    /// Creating a child under the MOUNT node is the shape a user drives: after
    /// a share the page the UI navigates to is the mount, so `parent_id` is the
    /// mount's id, not the shared root's. The new child belongs to the shared
    /// doc under the shared root — landing it in the global doc means it never
    /// reaches the peer.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn create_under_mount_node_lands_in_shared_doc() {
        use holon_api::repository::CoreOperations;
        let (backend_a, backend_b, write_backend, shared_tree_id, _da, _db) = share_setup().await;
        let mount_uri = mount_uri_of(&backend_a).await;

        write_backend
            .create_block(
                // ALLOW(entity_uri_from_raw): mount uri built from a TreeID above
                EntityUri::from_raw(&mount_uri),
                holon_api::BlockContent::text("born under the mount"),
                Some(EntityUri::block("mount-born-child")),
            )
            .await
            .expect("create under a mount node must succeed");

        assert_eq!(
            shared_text(&backend_a, &shared_tree_id, "mount-born-child").as_deref(),
            Some("born under the mount"),
            "a child created under the mount must land in the SHARED doc"
        );
        let a_global = backend_a.global_doc().await.unwrap();
        assert!(
            a_global
                .with_read(|doc| Ok(find_tree_id_by_stable_id(
                    doc,
                    &EntityUri::block("mount-born-child")
                )))
                .unwrap()
                .is_none(),
            "the child must NOT land in the global doc (it would never reach the peer)"
        );

        let synced = backend_b.sync_with_peers(&shared_tree_id).await.unwrap();
        assert_eq!(synced, 1);
        assert_eq!(
            shared_text(&backend_b, &shared_tree_id, "mount-born-child").as_deref(),
            Some("born under the mount"),
            "B must see the created child after a pull"
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// Read/write symmetry on the mount id: the write path takes a mount as a
    /// parent and lands the child under the shared root, so listing the mount's
    /// children must answer with the shared root's children. Answering "none"
    /// would leave the two sides disagreeing about what the mount id means.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn list_children_of_the_mount_lists_the_shared_roots_children() {
        use holon_api::repository::CoreOperations;
        let (backend_a, backend_b, write_backend, _stid, _da, _db) = share_setup().await;
        let mount_uri = mount_uri_of(&backend_a).await;

        write_backend
            .create_block(
                // ALLOW(entity_uri_from_raw): mount uri built from a TreeID
                EntityUri::from_raw(&mount_uri),
                holon_api::BlockContent::text("added under the mount"),
                Some(EntityUri::block("mount-listed-child")),
            )
            .await
            .unwrap();

        let via_mount = write_backend.list_children(&mount_uri).await.unwrap();
        let via_shared_root = write_backend
            .list_children("block:shared-parent")
            .await
            .unwrap();
        assert_eq!(
            via_mount, via_shared_root,
            "the mount and the shared root must answer with the same children"
        );
        assert!(
            via_mount.contains(&"block:mount-listed-child".to_string()),
            "the child created under the mount must be listed under it, got {via_mount:?}"
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
        let mount_uri = mount_uri_of(&backend_a).await;

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
        let bus = Arc::new(ConditionBus::new());
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

    /// Set up a save worker over a store whose publish window is held
    /// open, commit once, and hand back everything the caller needs to
    /// observe the window. `doc` and `store` are returned so they stay
    /// alive for the duration of the test.
    async fn armed_save_worker(
        dir: &TempDir,
        stall: Duration,
    ) -> (Arc<SharedSnapshotStore>, Arc<LoroDoc>, SaveWorker) {
        let bus = Arc::new(ConditionBus::new());
        let store = Arc::new(SharedSnapshotStore::new(
            dir.path().to_path_buf(),
            bus.clone(),
        ));
        store.set_publish_stall(stall);
        let doc = Arc::new(LoroDoc::new());
        let worker = spawn_save_worker(store.clone(), bus, "stalled".to_string(), doc.clone());
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let node = tree.create(None::<TreeID>).unwrap();
        tree.get_meta(node).unwrap().insert("n", 1i64).unwrap();
        doc.commit();
        (store, doc, worker)
    }

    fn tmp_files(shares: &std::path::Path) -> Vec<std::path::PathBuf> {
        if !shares.exists() {
            return vec![];
        }
        std::fs::read_dir(shares)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.ends_with(".tmp"))
            })
            .collect()
    }

    /// The publish is `write tmp → fsync → rename`, so a `.loro.tmp`
    /// under `shares/` is a legal transient for as long as that call
    /// runs. A sweep that is only separated from the commit by a fixed
    /// sleep can therefore land inside the window and see the tmp —
    /// which is what `subtree_share_round_trip_pbt`'s `SettleSaves`
    /// used to do. Pins the mechanism, not a defect.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_fixed_sleep_can_land_inside_the_publish_window() {
        let dir = TempDir::new().unwrap();
        let (_store, _doc, worker) =
            armed_save_worker(&dir, SAVE_DEBOUNCE + Duration::from_millis(600)).await;

        // The settle policy `SettleSaves` used: sleep past
        // `SAVE_DEBOUNCE`, then sweep. Observed off the runtime because
        // the publish is a blocking call and holds tokio's timers with
        // it — one more reason a timer is the wrong settle point here.
        let shares = dir.path().join("shares");
        let observed = tokio::task::spawn_blocking(move || {
            std::thread::sleep(Duration::from_millis(400));
            tmp_files(&shares)
        })
        .await
        .unwrap();

        assert!(
            !observed.is_empty(),
            "expected the stalled publish to be observable as a .loro.tmp"
        );
        // The worker is correspondingly NOT idle — the settle point the
        // sweep should have used says so.
        assert!(!worker.handle.quiesce().is_idle());
    }

    /// Worker quiescence IS a settle point: once it resolves, the
    /// rename has completed, the snapshot is on disk, and nothing of
    /// ours is left under a `.tmp` name.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn worker_quiesce_is_a_publish_settle_point() {
        let dir = TempDir::new().unwrap();
        let (_store, _doc, worker) =
            armed_save_worker(&dir, SAVE_DEBOUNCE + Duration::from_millis(600)).await;

        tokio::time::timeout(Duration::from_secs(30), worker.handle.quiesce().wait_idle())
            .await
            .expect("save worker did not quiesce");

        assert_eq!(
            tmp_files(&dir.path().join("shares")),
            Vec::<std::path::PathBuf>::new(),
            "quiesced save worker left a .tmp behind"
        );
        assert!(dir.path().join("shares/stalled.loro").is_file());
    }

    /// A harness that corrupts a snapshot on disk needs the guarantee
    /// that nothing will rewrite it afterwards. The sync worker is one
    /// such writer: `sync_with_peers` republishes the snapshot as its
    /// save-before-push barrier BEFORE it dials, so a barrier save armed
    /// by an earlier commit lands on top of the corrupt bytes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn settling_including_sync_keeps_a_corrupt_snapshot_corrupt() {
        const CORRUPT: &[u8] = b"\x00\x01not-loro";

        let (backend, _dir) = make_backend();
        let id = "corrupt".to_string();
        backend.manager.register(id.clone(), LoroDoc::new());
        let doc = backend.manager.get_doc(&id).unwrap();
        backend.snapshot_store().save(&id, &doc).unwrap();
        backend.attach_sync_worker(id.clone(), doc.clone()).await;

        // A local commit arms the sync worker. No peers are registered,
        // so its work call is the barrier save and nothing else.
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let node = tree.create(None::<TreeID>).unwrap();
        tree.get_meta(node).unwrap().insert("n", 1i64).unwrap();
        doc.commit();

        tokio::time::timeout(
            Duration::from_secs(60),
            backend.wait_for_workers_idle(SettleScope::IncludingSync),
        )
        .await
        .expect("settle never returned");

        let path = backend.snapshot_store().snapshot_path(&id);
        std::fs::write(&path, CORRUPT).unwrap();
        tokio::time::sleep(SYNC_DEBOUNCE * 3).await;

        let on_disk = std::fs::read(&path).unwrap();
        assert!(
            on_disk == CORRUPT,
            "a barrier save republished over the corruption after the settle returned: \
             {} bytes on disk, expected the {}-byte corrupt payload",
            on_disk.len(),
            CORRUPT.len()
        );
    }

    /// Settling for a disk sweep must not cost what a network round
    /// trip costs. The sync worker's work call is `sync_with_peers`,
    /// which persists a barrier snapshot and then dials every known
    /// peer under a 30 s `CONNECT_TIMEOUT` each, so including it in
    /// quiescence would make the settle's duration a function of peer
    /// reachability rather than of pending local work.
    ///
    /// Both halves of that cost are present here: an unreachable peer
    /// address, and a publish stall that holds the barrier save open
    /// regardless of what the network does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn settle_stays_bounded_while_the_sync_worker_dials_an_unreachable_peer() {
        let (backend, _dir) = make_backend();
        backend
            .snapshot_store()
            .set_publish_stall(Duration::from_secs(5));

        let id = "unreachable".to_string();
        backend.manager.register(id.clone(), LoroDoc::new());
        let doc = backend.manager.get_doc(&id).unwrap();

        // TEST-NET-1 (RFC 5737). Never routed, so the dial either sits
        // until CONNECT_TIMEOUT or fails fast depending on the network;
        // the publish stall keeps the work call slow either way.
        let peer_key = iroh::SecretKey::generate(&mut rand::rng());
        let unreachable = EndpointAddr::from_parts(
            peer_key.public(),
            [iroh::TransportAddr::Ip("192.0.2.1:9".parse().unwrap())],
        );
        backend.remember_peer(&id, unreachable).await;

        backend.attach_sync_worker(id.clone(), doc.clone()).await;
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let node = tree.create(None::<TreeID>).unwrap();
        tree.get_meta(node).unwrap().insert("n", 1i64).unwrap();
        doc.commit();

        let started = std::time::Instant::now();
        tokio::time::timeout(
            Duration::from_secs(120),
            backend.wait_for_workers_idle(SettleScope::LocalWrites),
        )
        .await
        .expect("settle never returned");
        let elapsed = started.elapsed();

        // Non-vacuity: read before asserting, so a settle that returned
        // fast only because the worker was already done cannot pass.
        let still_busy = {
            let guard = backend.sync_workers.read().await;
            !guard
                .get(&id)
                .expect("sync worker registered")
                .handle
                .quiesce()
                .is_idle()
        };
        assert!(
            elapsed < Duration::from_secs(3),
            "settle took {elapsed:?}; it waited on the sync worker's barrier save \
             and its dial to an unreachable peer"
        );
        assert!(
            still_busy,
            "sync worker had already finished — the bound above proves nothing"
        );
    }

    /// Share A's subtree and return `(ticket, shared_tree_id)`.
    async fn share_and_ticket(backend: &LoroShareBackend, block: &str) -> (String, String) {
        let resp = backend
            .share_subtree(block, "none".into())
            .await
            .expect("share_subtree");
        let j: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            other => panic!("unexpected response type: {other:?}"),
        };
        (
            j["ticket"].as_str().unwrap().to_string(),
            j["shared_tree_id"].as_str().unwrap().to_string(),
        )
    }

    /// The single peer this share's roster has pinned.
    async fn only_enrolled_peer(
        backend: &LoroShareBackend,
        shared_tree_id: &str,
    ) -> PeerFingerprint {
        let roster = backend
            .advertiser_for_test()
            .roster_for(shared_tree_id)
            .await
            .expect("a share created through the lifecycle is advertised WITH a roster");
        let guard = roster.lock().await;
        let peers = guard.enrolled_peers();
        assert_eq!(
            peers.len(),
            1,
            "expected exactly one pinned peer, found {}",
            peers.len()
        );
        peers[0]
    }

    /// Append `content` to the shared subtree's root text on `backend`.
    async fn edit_shared_root(backend: &LoroShareBackend, shared_tree_id: &str, content: &str) {
        let doc = backend
            .manager_for_test()
            .get_doc(shared_tree_id)
            .expect("shared doc registered");
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let root = tree.roots()[0];
        let meta = tree.get_meta(root).unwrap();
        let text: loro::LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
        text.insert(0, content).unwrap();
        doc.commit();
    }

    fn shared_doc_debug(backend: &LoroShareBackend, shared_tree_id: &str) -> String {
        format!(
            "{:?}",
            backend
                .manager_for_test()
                .get_doc(shared_tree_id)
                .expect("shared doc registered")
                .get_deep_value()
        )
    }

    /// LIFECYCLE H5, the half the advertiser tests could not reach: a share
    /// created by `share_subtree` is GATED, so the leaky `shared_tree_id` —
    /// which travels in the mount node and in projected SQL rows — buys a
    /// stranger neither a read nor a write.
    ///
    /// The stranger is given everything a forged ticket carries: the ALPN
    /// (which IS the share id) and a dialable addr. It holds no capability.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_stranger_holding_only_the_shared_tree_id_can_neither_read_nor_write_the_share() {
        use crate::iroh_sync_adapter::sync_doc_initiate;

        let (backend_a, _dir_a) = make_backend();
        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "confidential-payload").await;

        let (_ticket, shared_tree_id) = share_and_ticket(&backend_a, "block:shared").await;

        // Non-vacuity: the content really is in the share, so "the stranger
        // did not get it" is a fact about the gate, not about an empty doc.
        assert!(
            shared_doc_debug(&backend_a, &shared_tree_id).contains("confidential-payload"),
            "the shared replica must hold the content the stranger is denied"
        );

        let addr = backend_a
            .advertiser_for_test()
            .endpoint_for(&shared_tree_id)
            .await
            .expect("the share is advertised")
            .addr();
        let alpn = make_alpn(ALPN_PREFIX, &shared_tree_id);

        let stranger_doc = Arc::new(LoroDoc::new());
        stranger_doc.set_peer_id(9_999).unwrap();
        {
            let tree = stranger_doc.get_tree(crate::loro_backend::TREE_NAME);
            tree.enable_fractional_index(0);
            let node = tree.create(None::<TreeID>).unwrap();
            let meta = tree.get_meta(node).unwrap();
            let text: loro::LoroText = meta.ensure_mergeable_text("content_raw").unwrap();
            text.insert(0, "stranger-graffiti").unwrap();
        }
        stranger_doc.commit();

        let ep = create_endpoint(vec![alpn.clone()]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let dialed = sync_doc_initiate(
            &ep,
            &stranger_doc,
            &alpn,
            addr,
            &shared_tree_id,
            Capabilities::read_write(),
        )
        .await;
        // Whether the dial errs or is simply hung up on is the acceptor's
        // business; the property is what crossed.
        let _ = dialed;
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(
            !format!("{:?}", stranger_doc.get_deep_value()).contains("confidential-payload"),
            "an un-rostered peer must not receive the shared subtree"
        );
        assert!(
            !shared_doc_debug(&backend_a, &shared_tree_id).contains("stranger-graffiti"),
            "an un-rostered peer's ops must not enter the shared replica"
        );
        assert_eq!(
            backend_a
                .advertiser_for_test()
                .roster_for(&shared_tree_id)
                .await
                .expect("gated share")
                .lock()
                .await
                .enrolled_count(),
            0,
            "a peer that proved nothing must not be pinned"
        );

        // CONTROL, in the same test against the same live endpoint: a peer
        // that holds the TICKET's capability — the only thing the stranger
        // lacked — does get the subtree. Without this, "the stranger received
        // nothing" would also hold for a share nobody could reach at all.
        let ticket = Ticket::decode(&_ticket).expect("decode our own ticket");
        let holder_doc = Arc::new(LoroDoc::new());
        holder_doc.set_peer_id(8_888).unwrap();
        let holder_ep = create_endpoint(vec![alpn.clone()]).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        sync_doc_initiate_enrolled(
            &holder_ep,
            &holder_doc,
            &alpn,
            ticket.addr.clone(),
            &ticket.capability,
            &shared_tree_id,
            Capabilities::read_write(),
        )
        .await
        .expect("a peer holding the ticket's capability must enroll and sync");
        assert!(
            format!("{:?}", holder_doc.get_deep_value()).contains("confidential-payload"),
            "control: the capability holder must receive what the stranger was denied"
        );
    }

    /// Revocation is only revocation if it stops BOTH legs: the revoked peer
    /// cannot dial in (un-pinned, and the enrollment window is closed so the
    /// capability it kept cannot re-admit it), and this device stops dialing
    /// it — otherwise its ops would keep arriving through our own pull.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn revoking_a_peer_stops_every_further_import_from_it() {
        let (backend_a, _dir_a) = make_backend();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "base").await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let (ticket, shared_tree_id) = share_and_ticket(&backend_a, "block:shared").await;
        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;

        // Non-vacuity: an enrolled peer's edit DOES reach A, so the negative
        // below is about the revocation and not about a dead sync path.
        edit_shared_root(&backend_b, &shared_tree_id, "before-revocation-").await;
        backend_b.sync_with_peers(&shared_tree_id).await.unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert!(
            shared_doc_debug(&backend_a, &shared_tree_id).contains("before-revocation-"),
            "an enrolled peer's edit must reach the sharer"
        );

        let b_peer = only_enrolled_peer(&backend_a, &shared_tree_id).await;
        assert!(
            backend_a
                .revoke_share_peer(&shared_tree_id, b_peer)
                .await
                .unwrap(),
            "the peer was enrolled, so revoking it must report a removal"
        );

        edit_shared_root(&backend_b, &shared_tree_id, "after-revocation-").await;
        // B still holds the capability and still knows A's addr — the two
        // things revocation must survive.
        let synced = backend_b.sync_with_peers(&shared_tree_id).await.unwrap();
        assert_eq!(synced, 0, "a revoked peer's dial must not complete a round");
        // And A must not pull it either.
        let _ = backend_a.sync_with_peers(&shared_tree_id).await;
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert!(
            !shared_doc_debug(&backend_a, &shared_tree_id).contains("after-revocation-"),
            "no op authored by a revoked peer may enter the replica afterwards"
        );
    }

    /// Revocation has to survive a restart, and the acceptor half does so on
    /// its own (the roster sidecar is rewritten). The OUTBOUND half is the one
    /// that can quietly come back: `sync_with_peers` dials every addr in
    /// `known_peers`, and that set is reloaded from the peers sidecar at
    /// rehydrate. If the revocation only dropped the in-memory copy, the next
    /// launch would dial the revoked peer again and pull its ops.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_revoked_peer_is_not_re_dialed_after_a_restart() {
        let (backend_a, keychain_a, dir_a) = make_backend_with_keychain();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "base").await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let (ticket, shared_tree_id) = share_and_ticket(&backend_a, "block:shared").await;
        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;

        let b_peer = only_enrolled_peer(&backend_a, &shared_tree_id).await;
        // Non-vacuity: A really would dial B, so "A does not dial B after the
        // restart" is about the revocation and not about an empty peer set.
        assert_eq!(
            backend_a.known_peers_for_test(&shared_tree_id).await.len(),
            1,
            "A must know B before the revocation, or the assertion below is vacuous"
        );

        assert!(
            backend_a
                .revoke_share_peer(&shared_tree_id, b_peer)
                .await
                .unwrap(),
            "the peer was enrolled, so revoking it must report a removal"
        );

        // Restart A over the same storage dir and keychain — a real relaunch.
        backend_a.advertiser_for_test().close_all().await;
        backend_a.flush_all().await;
        let dir_a_path = dir_a.path().to_path_buf();
        drop(backend_a);
        backend_b.advertiser_for_test().close_all().await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        let backend_a = make_backend_at(&dir_a_path, test_credentials(keychain_a));
        assert_eq!(
            rehydrate_over(&backend_a).await,
            1,
            "A must rehydrate its one share"
        );

        let dialable = backend_a.known_peers_for_test(&shared_tree_id).await;
        assert!(
            !dialable
                .iter()
                .any(|addr| PeerFingerprint::from_bytes(*addr.id.as_bytes()) == b_peer),
            "the revoked peer came back as a dial target after the restart: {dialable:?}"
        );

        backend_a.advertiser_for_test().close_all().await;
    }

    /// A revocation whose peers sidecar cannot be rewritten did NOT hold: the
    /// in-memory drop dies with the process and the next launch dials the
    /// revoked peer again. So the failure is an `Err` naming the peer, not a
    /// `warn!` the caller can neither see nor act on.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_revocation_that_cannot_be_persisted_fails_loudly_naming_the_peer() {
        let (backend_a, _keychain_a, _dir_a) = make_backend_with_keychain();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "base").await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let (ticket, shared_tree_id) = share_and_ticket(&backend_a, "block:shared").await;
        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let b_peer = only_enrolled_peer(&backend_a, &shared_tree_id).await;

        // Block the sidecar's atomic write by occupying its published path
        // with a directory: the rename onto it fails, so `save_peers` errs.
        // The roster sidecar written earlier in `revoke_share_peer` has its
        // own name and is untouched.
        let blocker = backend_a.snapshot_store().peers_path(&shared_tree_id);
        let _ = std::fs::remove_file(&blocker);
        std::fs::create_dir_all(&blocker).unwrap();

        let err = backend_a
            .revoke_share_peer(&shared_tree_id, b_peer)
            .await
            .expect_err("a revocation that cannot be persisted must not report success");
        let msg = format!("{err:#}");
        assert!(msg.contains(&shared_tree_id), "{msg}");
        assert!(
            msg.contains(&format!("{b_peer:?}")),
            "the error must name the peer whose revocation did not stick: {msg}"
        );

        backend_a.advertiser_for_test().close_all().await;
        backend_b.advertiser_for_test().close_all().await;
    }

    /// Build a peer addr plus the fingerprint the roster pins it under.
    fn synthetic_peer() -> (EndpointAddr, crate::share_enrollment::PeerFingerprint) {
        let addr = EndpointAddr::new(iroh::SecretKey::generate(&mut rand::rng()).public());
        let fp = crate::share_enrollment::PeerFingerprint::from_bytes(*addr.id.as_bytes());
        (addr, fp)
    }

    /// The admission callback persists the peers sidecar from a spawned task,
    /// so it overlaps a revocation persisting the same sidecar. The revocation
    /// must still land: an `Err` here is a revocation the caller must treat as
    /// "did not stick", and the peer stays dialable after the next launch.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_concurrent_admission_does_not_fail_a_revocations_sidecar_write() {
        let (backend, _dir) = make_backend();
        let id = "tree-revocation-race".to_string();
        let (revoked, revoked_fp) = synthetic_peer();
        let (other, _) = synthetic_peer();
        backend.remember_peer(&id, revoked.clone()).await;

        backend
            .snapshot_store()
            .stall_next_peers_publish(Duration::from_millis(500));
        let revoking = {
            let backend = Arc::clone(&backend);
            let id = id.clone();
            tokio::spawn(async move { backend.forget_peer_addrs(&id, &revoked_fp).await })
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        backend.remember_peer(&id, other).await;

        revoking
            .await
            .unwrap()
            .expect("a concurrent admission must not make the revocation's sidecar write fail");
    }

    /// The mirror interleaving: the admission is the slower writer. It
    /// snapshots `known_peers` BEFORE the revocation removes the peer, so
    /// if the disk write happens outside that lock the admission's stale
    /// copy lands last and puts the revoked peer's dial addr back — with
    /// both operations reporting success. `sync_with_peers` dials the
    /// sidecar after a restart, so that is a silently undone revocation.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_concurrent_admission_cannot_restore_a_revoked_peers_addr_on_disk() {
        let (backend, _dir) = make_backend();
        let id = "tree-admission-race".to_string();
        let (revoked, revoked_fp) = synthetic_peer();
        backend.remember_peer(&id, revoked.clone()).await;

        backend
            .snapshot_store()
            .stall_next_peers_publish(Duration::from_millis(500));
        let admitting = {
            let backend = Arc::clone(&backend);
            let id = id.clone();
            let addr = revoked.clone();
            tokio::spawn(async move { backend.remember_peer(&id, addr).await })
        };
        tokio::time::sleep(Duration::from_millis(150)).await;
        backend.forget_peer_addrs(&id, &revoked_fp).await.unwrap();
        admitting.await.unwrap();

        let on_disk = backend.snapshot_store().load_peers(&id).unwrap();
        assert!(
            !on_disk.iter().any(|a| a.id == revoked.id),
            "a revoked peer's dial addr must not survive in the sidecar: {on_disk:?}"
        );
    }

    /// Fail CLOSED at rehydrate: a share whose roster cannot be rebuilt (the
    /// keychain entry holding its capability is gone) is NOT advertised.
    /// Advertising it would serve the leaky `shared_tree_id` to every peer that
    /// can route here — the exact hole this lane closes — so the share stays
    /// registered and pullable, and the refusal is disclosed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_share_whose_roster_cannot_be_rebuilt_is_not_advertised() {
        let (backend_a, keychain_a, dir_a) = make_backend_with_keychain();
        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "confidential-payload").await;
        let (_ticket, shared_tree_id) = share_and_ticket(&backend_a, "block:shared").await;

        backend_a.advertiser_for_test().close_all().await;
        backend_a.flush_all().await;
        let dir_a_path = dir_a.path().to_path_buf();
        drop(backend_a);
        tokio::time::sleep(Duration::from_millis(200)).await;

        // CONTROL first, over the same storage dir: with the keychain intact
        // the restart DOES advertise, gated. Without it, "not advertised"
        // would also hold for a share that simply failed to rehydrate.
        {
            let control = make_backend_at(&dir_a_path, test_credentials(keychain_a));
            assert_eq!(rehydrate_over(&control).await, 1);
            assert!(
                control
                    .advertiser_for_test()
                    .roster_for(&shared_tree_id)
                    .await
                    .is_some(),
                "control: with its capability the share rehydrates GATED"
            );
            control.advertiser_for_test().close_all().await;
            control.flush_all().await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        // The lost keychain: same shares on disk, no capability filed.
        let lost = make_backend_at(&dir_a_path, test_credentials(test_keychain()));
        let mut changes = lost.degraded_bus().subscribe().changes;
        assert_eq!(
            rehydrate_over(&lost).await,
            1,
            "the share still rehydrates — it is usable for pulls, just not served"
        );

        assert!(
            !lost.advertiser_for_test().is_active(&shared_tree_id).await,
            "a share whose roster could not be rebuilt must NOT be advertised — serving it \
             would admit whoever knows the shared_tree_id"
        );
        assert!(
            lost.advertiser_for_test()
                .roster_for(&shared_tree_id)
                .await
                .is_none()
        );

        let mut disclosed = Vec::new();
        while let Ok(change) = changes.try_recv() {
            if let Some(event) = change.raised()
                && let ConditionKind::RehydrationFailed(detail) = &event.reason
                && event.subject == shared_tree_id
            {
                disclosed.push(detail.clone());
            }
        }
        assert!(
            disclosed.iter().any(|d| d.contains("share not advertised")),
            "the refusal must be disclosed, not silent: {disclosed:?}"
        );

        lost.advertiser_for_test().close_all().await;
    }

    /// The owner identity key is minted on the FIRST share and its one-time
    /// recovery code is dropped — nothing can show it. That is disclosed on
    /// the degraded bus (the code itself never is), once: a second share
    /// reloads the same key and has nothing new to say.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn the_dropped_owner_recovery_code_is_disclosed_once_on_the_first_share() {
        let (backend, _dir) = make_backend();
        let mut changes = backend.degraded_bus().subscribe().changes;

        seed_block(&backend, "root-a", None, "root-a").await;
        seed_block(&backend, "first", Some("root-a"), "first").await;
        seed_block(&backend, "second", Some("root-a"), "second").await;

        let (_t1, _id1) = share_and_ticket(&backend, "block:first").await;
        let (_t2, _id2) = share_and_ticket(&backend, "block:second").await;

        let mut disclosures = 0usize;
        while let Ok(change) = changes.try_recv() {
            if let Some(event) = change.raised()
                && matches!(event.reason, ConditionKind::OwnerRecoveryCodeNotShown)
            {
                assert_eq!(
                    event.subject,
                    holon_api::condition_bus::OWNER_IDENTITY_SUBJECT
                );
                disclosures += 1;
            }
        }
        assert_eq!(
            disclosures, 1,
            "the dropped recovery code is disclosed on the mint only — the second share reloads \
             the same key"
        );

        backend.advertiser_for_test().close_all().await;
    }

    /// The roster is durable state, not session state: a restart rebuilds it
    /// from the keychain capability plus the owner-signed sidecar, so a peer
    /// admitted before the restart reconnects WITHOUT re-proving anything.
    ///
    /// Proven by the short-circuit in `ShareRoster::authorize`: an
    /// already-pinned peer is authorized with a garbage proof (`newly_enrolled
    /// == false`), while an unknown peer presenting the same garbage is
    /// refused. If the pinned set had been lost, the first call would fail.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_restart_reloads_the_roster_so_an_enrolled_peer_need_not_re_enroll() {
        use crate::share_enrollment::CapabilityId;
        use crate::share_enrollment::Challenge;
        use crate::share_enrollment::ProofTag;

        let (backend_a, keychain_a, dir_a) = make_backend_with_keychain();
        let (backend_b, _dir_b) = make_backend();

        seed_block(&backend_a, "root-a", None, "root-a").await;
        seed_block(&backend_a, "shared", Some("root-a"), "base").await;
        seed_block(&backend_b, "root-b", None, "root-b").await;

        let (ticket, shared_tree_id) = share_and_ticket(&backend_a, "block:shared").await;
        backend_b
            .accept_shared_subtree("block:root-b", ticket)
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(500)).await;
        let b_peer = only_enrolled_peer(&backend_a, &shared_tree_id).await;

        // Restart A over the same storage dir and the same keychain.
        backend_a.advertiser_for_test().close_all().await;
        backend_a.flush_all().await;
        let dir_a_path = dir_a.path().to_path_buf();
        drop(backend_a);
        tokio::time::sleep(Duration::from_millis(200)).await;

        let backend_a = make_backend_at(&dir_a_path, test_credentials(keychain_a));
        let rehydrated = rehydrate_over(&backend_a).await;
        assert_eq!(rehydrated, 1, "A must rehydrate its one share");

        let roster = backend_a
            .advertiser_for_test()
            .roster_for(&shared_tree_id)
            .await
            .expect("a rehydrated share must be advertised WITH its roster, never un-gated");
        let mut guard = roster.lock().await;
        assert!(
            guard.is_enrolled(&b_peer),
            "the pinned peer set must survive the restart"
        );

        let now = chrono::Utc::now().timestamp();
        let garbage_id = CapabilityId::from_bytes([0u8; 32]);
        let garbage_proof = ProofTag::from_bytes([0u8; 32]);
        let authorized = guard
            .authorize(
                now,
                &Challenge::generate(),
                &garbage_id,
                &garbage_proof,
                b_peer,
            )
            .expect("an already-pinned peer reconnects without re-proving the capability");
        assert!(
            !authorized.newly_enrolled(),
            "the peer must be recognised as already enrolled, not admitted afresh"
        );
        assert!(
            guard
                .authorize(
                    now,
                    &Challenge::generate(),
                    &garbage_id,
                    &garbage_proof,
                    PeerFingerprint::from_bytes([42u8; 32]),
                )
                .is_err(),
            "the short-circuit must apply to pinned peers only — an unknown peer with the same \
             garbage proof must still be refused"
        );
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
        let (backend_a, keychain_a, dir_a) = make_backend_with_keychain();
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

        // Same storage dir AND same keychain: a restart keeps both, and the
        // rehydrated roster needs the capability secret from the second.
        let backend_a = make_backend_at(&dir_a_path, test_credentials(keychain_a));
        let collab = backend_a.test_global_doc().await;
        // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
        // exists to observe.
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

        let (backend_a, keychain_a, dir_a) = make_backend_with_keychain();
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

        // Same storage dir AND same keychain: a restart keeps both, and the
        // rehydrated roster needs the capability secret from the second.
        let backend_a = make_backend_at(&dir_a_path, test_credentials(keychain_a));
        let collab = backend_a.test_global_doc().await;
        // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
        // exists to observe.
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
    /// and assert the worker emits `Condition::SnapshotSaveFailed`
    /// while keeping the in-memory doc's edit intact.
    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn readonly_shares_dir_emits_degraded() {
        use std::os::unix::fs::PermissionsExt;

        let dir = TempDir::new().unwrap();
        let bus = Arc::new(ConditionBus::new());
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
            Err(_) => panic!("no Condition event within 1s"),
        };
        assert_eq!(ev.subject, "readonly");
        assert!(matches!(ev.reason, ConditionKind::SnapshotSaveFailed(_)));

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
    /// with the UPSERT create semantics of the SQL `block` table the
    /// projection targets. Lets backend-only tests assert what the sharer
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
                "delete" => {
                    rows.remove(&id);
                }
                // create is an UPSERT; update / set_field merge the written
                // columns into the row, as an SQL UPDATE does.
                _ => {
                    rows.entry(id).or_default().extend(params);
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
            operations: Vec<holon_core::BatchOp>,
            _: crate::event_bus::EventOrigin,
        ) -> Result<Vec<OperationResult>> {
            let mut out = Vec::with_capacity(operations.len());
            for op in operations {
                self.apply(&op.op_name, op.params);
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
            _: Vec<holon_core::BatchOp>,
            _: crate::event_bus::EventOrigin,
        ) -> Result<Vec<OperationResult>> {
            Err(err("stub sql: batch write failure"))
        }
    }

    /// `DownstreamProjection` that reports a fixed outcome — the global
    /// projection's answer to `share_subtree`'s prune-delete barrier.
    struct FixedPassProjection(holon_core::ProjectionPass);

    #[async_trait]
    impl holon_core::DownstreamProjection for FixedPassProjection {
        async fn flush(&self) -> holon_core::traits::Result<holon_core::ProjectionPass> {
            Ok(self.0)
        }

        async fn consolidator_behind_sink(&self) -> holon_core::traits::Result<bool> {
            Ok(false)
        }
    }

    /// Like `make_backend`, but wires a `RecordingSqlOps` so mount/descendant
    /// projection writes are observable. `downstream_projection` is `None` —
    /// there is no global `LoroSyncController` loop in these tests, so no
    /// prune-delete races the projection and the flush barrier is unnecessary.
    fn make_backend_with_sql() -> (Arc<LoroShareBackend>, Arc<RecordingSqlOps>, TempDir) {
        make_backend_with_sql_and_projection(None)
    }

    fn make_backend_with_sql_and_projection(
        projection: Option<Arc<dyn DownstreamProjection>>,
    ) -> (Arc<LoroShareBackend>, Arc<RecordingSqlOps>, TempDir) {
        let (backend, sql, dir) = build_backend(projection, None);
        (backend, sql, dir)
    }

    /// The recipe fixture every read-only-tier test here shares: one
    /// `.cook` document whose declared membership is a single step.
    fn recipe_documents() -> (Arc<holon_core::ReadOnlyDocuments>, EntityUri) {
        let path = std::path::Path::new("/vault/Pancakes.cook");
        let step = EntityUri::block("Pancakes.cook::b::0");
        let documents = Arc::new(holon_core::ReadOnlyDocuments::new());
        documents.record(
            &EntityUri::block("Pancakes.cook"),
            "cooklang",
            path,
            &holon_core::ReadOnlyMembers::from_persisted_row(path, vec![step.clone()])
                .expect("a non-empty membership"),
        );
        (documents, step)
    }

    /// `make_backend_with_sql`, plus the write-tier authority over a recipe
    /// document — the wiring a vault holding one `.cook` file produces.
    fn make_backend_with_tier() -> (
        Arc<LoroShareBackend>,
        Arc<RecordingSqlOps>,
        Arc<holon_core::ReadOnlyDocuments>,
        EntityUri,
        TempDir,
    ) {
        let (documents, step) = recipe_documents();
        let tier = Arc::new(TestTier(documents.clone())) as Arc<dyn WriteTierAuthority>;
        let (backend, sql, dir) = build_backend(None, Some(tier));
        (backend, sql, documents, step, dir)
    }

    /// Plant a mount for `shared_tree_id` in `global` — under `parent`, or at
    /// the root — with stable id `mount`, the way an accept does, and describe
    /// the share it places.
    fn plant_mount(
        global: &crate::loro_document::LoroDocument,
        shared_tree_id: &str,
        parent: Option<TreeID>,
    ) -> ShareRoot {
        // ALLOW(loro_doc_escape): single-threaded test setup; no concurrent writer
        // exists to observe.
        let doc = global.doc();
        let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
        let mount =
            shared_tree::create_mount_node(&tree, parent, shared_tree_id, TreeID::new(u64::MAX, 0))
                .unwrap();
        set_stable_id(&doc, mount, "mount").unwrap();
        doc.commit();
        ShareRoot {
            mount,
            root_is_page: false,
        }
    }

    fn build_backend(
        projection: Option<Arc<dyn DownstreamProjection>>,
        write_tier: Option<Arc<dyn WriteTierAuthority>>,
    ) -> (Arc<LoroShareBackend>, Arc<RecordingSqlOps>, TempDir) {
        let dir = TempDir::new().unwrap();
        let store = Arc::new(RwLock::new(LoroDocumentStore::new(
            dir.path().to_path_buf(),
        )));
        let bus = Arc::new(ConditionBus::new());
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
            test_credentials(test_keychain()),
            Some(sql.clone() as Arc<dyn OriginTaggedWrites>),
            projection,
            write_tier,
        );
        (backend, sql, dir)
    }

    /// Fix 2: a projection worker whose `sql_ops` batch fails must emit a
    /// `Condition { SqlProjectionFailed }` (not just log), so the UI can
    /// surface the Loro↔SQL divergence.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn projection_worker_failure_emits_degraded() {
        let bus = Arc::new(ConditionBus::new());
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
            "proj-share".to_string(),
            plant_mount(&global, "proj-share", None),
            global,
            None,
        )
        .expect("the share has a mount to place it");

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
            Err(_) => panic!("no Condition event within 2s"),
        };
        assert_eq!(ev.subject, "proj-share");
        assert!(
            matches!(ev.reason, ConditionKind::SqlProjectionFailed(_)),
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

        let bus = Arc::new(ConditionBus::new());
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
            // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
            // exists to observe.
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
            "hostile-share".to_string(),
            plant_mount(&global, "hostile-share", None),
            global,
            None,
        )
        .expect("the share has a mount to place it");

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
            Err(_) => panic!("no Condition event within 2s"),
        };
        assert_eq!(ev.subject, "hostile-share");
        assert!(
            matches!(
                &ev.reason,
                ConditionKind::ForeignIdCollision(id) if id == "block:journals"
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

    /// Stands for the composition root's `ReadOnlyFormatGate` — the same
    /// authority the dispatcher and the editor's text cell consult.
    struct TestTier(Arc<holon_core::ReadOnlyDocuments>);

    #[async_trait]
    impl WriteTierAuthority for TestTier {
        fn any_read_only_documents(&self) -> bool {
            !self.0.is_empty()
        }

        async fn refusal_for(&self, block_id: &str) -> Result<Option<holon_core::EditRefused>> {
            Ok(self.0.refusal_for_block(&EntityUri::parse(block_id)?))
        }

        async fn adopt_sync_import(&self, block_id: &str, parent_id: &str) -> Result<bool> {
            Ok(self
                .0
                .adopt(&EntityUri::parse(parent_id)?, &EntityUri::parse(block_id)?))
        }

        fn disclose(&self, _: &holon_core::EditRefused) {}
    }

    /// Residual #1 of bugfunnel entry
    /// `2026-09-08-a-synced-block-under-a-read-only-document-stays-editable`,
    /// on the leg a real peer's blocks actually travel.
    ///
    /// The sharer shared a step of a `.cook` file; a peer added a block under
    /// it and synced back. The projection worker writes that import straight
    /// to the SQL block provider, never through the operation dispatcher, so
    /// the dispatcher's `OpOrigin::Sync` branch never judges it. The import
    /// must still LAND — the merge already happened in the peer — and it must
    /// inherit the document's tier, or the recipient ends up with a fully
    /// editable block that no writer can ever put into the recipe file.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_peer_import_under_a_read_only_homed_block_is_adopted_not_left_editable() {
        use crate::loro_backend::TREE_NAME;

        let path = std::path::Path::new("/vault/Pancakes.cook");
        let (documents, step) = recipe_documents();

        let bus = Arc::new(ConditionBus::new());
        let sql = Arc::new(RecordingSqlOps::default());
        // `share_subtree` prunes the shared subtree from the global tree, so
        // the collision guard finds no live local node for these ids.
        let global =
            Arc::new(crate::loro_document::LoroDocument::new("ro-global".to_string()).unwrap());

        let doc = Arc::new(LoroDoc::new());
        let _worker = spawn_projection_worker(
            doc.clone(),
            sql.clone() as Arc<dyn OriginTaggedWrites>,
            bus.clone(),
            "ro-share".to_string(),
            plant_mount(&global, "ro-share", None),
            global,
            Some(Arc::new(TestTier(documents.clone())) as Arc<dyn WriteTierAuthority>),
        )
        .expect("the share has a mount to place it");

        // The peer's update: the shared root IS the recipe step, and the peer
        // hung a new block under it.
        {
            let tree = doc.get_tree(TREE_NAME);
            let root = tree.create(None::<TreeID>).unwrap();
            let root_meta = tree.get_meta(root).unwrap();
            root_meta
                .insert(STABLE_ID, loro::LoroValue::from("Pancakes.cook::b::0"))
                .unwrap();
            let root_text: loro::LoroText = root_meta.ensure_mergeable_text("content_raw").unwrap();
            root_text.insert(0, "Crack the eggs").unwrap();

            let child = tree.create(Some(root)).unwrap();
            let child_meta = tree.get_meta(child).unwrap();
            child_meta
                .insert(STABLE_ID, loro::LoroValue::from("peer-added"))
                .unwrap();
            let child_text: loro::LoroText =
                child_meta.ensure_mergeable_text("content_raw").unwrap();
            child_text.insert(0, "and whisk them").unwrap();
            doc.commit();
        }

        let imported = EntityUri::block("peer-added");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while sql.get(imported.as_str()).is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the peer's block never reached SQL — a sync import must land, not be refused"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }
        let row = sql.get(imported.as_str()).expect("the row just appeared");
        assert_eq!(
            row.get("parent_id").and_then(|v| v.as_string()),
            Some(step.as_str()),
            "the import must keep the recipe step as its parent"
        );

        let refusal = documents.refusal_for_block(&imported).unwrap_or_else(|| {
            panic!(
                "{imported} was imported under {step}, a block of a read-only-homed document, and \
                 earns no refusal — every local edit to it is accepted and none of them can ever \
                 reach {}",
                path.display()
            )
        });
        assert!(
            refusal.to_string().contains("Pancakes.cook"),
            "the refusal must name the file, got: {refusal}"
        );
    }

    /// PROBE, RED BY DESIGN. Bugfunnel entry
    /// `2026-09-11-a-sync-import-projected-before-its-document-is-recorded-stays-editable`.
    ///
    /// The share projection can run BEFORE the file-sync controller records the
    /// document's read-only home, and nothing re-judges an import once it has
    /// landed. The membership `record` installs is derived from the FILE, which
    /// never declares an imported block, so the block stays editable for the
    /// rest of the vault's life with no disclosure at all.
    ///
    /// `#[ignore]` rather than left failing: the fix is not local to this
    /// crate. `ReadOnlyDocuments` holds no block tree, so `record` cannot find
    /// the children an earlier import placed under the blocks it is claiming;
    /// closing it needs an owner for "imports this device has taken", which is
    /// the same store Residual #2 of the 2026-09-08 entry asks for.
    /// Run it with `cargo nextest run -p holon-loro --run-ignored all`.
    #[ignore = "documented gap: bugfunnel 2026-09-11-a-sync-import-projected-before-its-document-is-recorded-stays-editable"]
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_import_projected_before_its_document_is_recorded_is_never_re_judged() {
        use crate::loro_backend::TREE_NAME;

        // The vault has not ingested the recipe yet: nothing is recorded.
        let path = std::path::Path::new("/vault/Pancakes.cook");
        let step = EntityUri::block("Pancakes.cook::b::0");
        let documents = Arc::new(holon_core::ReadOnlyDocuments::new());

        let bus = Arc::new(ConditionBus::new());
        let sql = Arc::new(RecordingSqlOps::default());
        let global =
            Arc::new(crate::loro_document::LoroDocument::new("early-global".to_string()).unwrap());
        let doc = Arc::new(LoroDoc::new());
        let _worker = spawn_projection_worker(
            doc.clone(),
            sql.clone() as Arc<dyn OriginTaggedWrites>,
            bus.clone(),
            "early-share".to_string(),
            plant_mount(&global, "early-share", None),
            global,
            Some(Arc::new(TestTier(documents.clone())) as Arc<dyn WriteTierAuthority>),
        )
        .expect("the share has a mount to place it");

        {
            let tree = doc.get_tree(TREE_NAME);
            let root = tree.create(None::<TreeID>).unwrap();
            let root_meta = tree.get_meta(root).unwrap();
            root_meta
                .insert(STABLE_ID, loro::LoroValue::from("Pancakes.cook::b::0"))
                .unwrap();
            let child = tree.create(Some(root)).unwrap();
            let child_meta = tree.get_meta(child).unwrap();
            child_meta
                .insert(STABLE_ID, loro::LoroValue::from("peer-added-early"))
                .unwrap();
            let child_text: loro::LoroText =
                child_meta.ensure_mergeable_text("content_raw").unwrap();
            child_text.insert(0, "and whisk them").unwrap();
            doc.commit();
        }

        let imported = EntityUri::block("peer-added-early");
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
        while sql.get(imported.as_str()).is_none() {
            assert!(
                tokio::time::Instant::now() < deadline,
                "the peer's block never reached SQL"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        }

        // Now the ingest runs and the document's home is recorded.
        documents.record(
            &EntityUri::block("Pancakes.cook"),
            "cooklang",
            path,
            &holon_core::ReadOnlyMembers::from_persisted_row(path, vec![step.clone()])
                .expect("a non-empty membership"),
        );

        assert!(
            documents.refusal_for_block(&imported).is_some(),
            "{imported} sits under {step} and the recipe is now recorded, yet it earns no \
             refusal — the import landed before the ingest and nothing re-judges it"
        );
    }

    /// The MOUNT leg. A share accepted UNDER a block of a read-only-homed
    /// document puts the mount node itself inside that document, so the mount
    /// inherits the tier — and every descendant projected beneath it inherits
    /// it through the mount (see the sibling test below).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_mount_projected_under_a_read_only_homed_parent_is_adopted() {
        let (backend, sql, documents, step, _dir) = make_backend_with_tier();
        let mount = "block:mount-in-recipe";

        backend
            .project_container_to_sql(mount, step.as_str(), "stid-mount")
            .await
            .expect("the mount projection must land");

        assert!(
            sql.get(mount).is_some(),
            "the mount row must land — an accept under a recipe step is not refused"
        );
        let mount_uri = EntityUri::parse(mount).expect("a valid mount uri");
        assert!(
            documents.refusal_for_block(&mount_uri).is_some(),
            "{mount} was mounted under {step}, a block of a read-only-homed document, and earns \
             no refusal — the mount is editable and nothing can write it back"
        );
    }

    /// The DESCENDANT leg (`accept_shared_subtree` / `rehydrate_shared_trees`).
    /// The mount is already bound to the recipe; the blocks projected beneath
    /// it inherit the binding through it, which is the chain
    /// `ReadOnlyDocuments::adopt` follows when a parent is itself adopted.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn descendants_projected_under_an_adopted_mount_inherit_its_read_only_home() {
        use crate::loro_backend::TREE_NAME;

        let (backend, sql, documents, step, _dir) = make_backend_with_tier();
        let mount = "block:mount-in-recipe";
        let mount_uri = EntityUri::parse(mount).expect("a valid mount uri");
        // The mount leg already bound the mount; this test pins the descendant
        // leg alone, so it seeds that binding directly.
        assert!(documents.adopt(&step, &mount_uri), "the mount binds first");

        let shared = LoroDoc::new();
        {
            let tree = shared.get_tree(TREE_NAME);
            let root = tree.create(None::<TreeID>).unwrap();
            let root_meta = tree.get_meta(root).unwrap();
            root_meta
                .insert(STABLE_ID, loro::LoroValue::from("shared-root"))
                .unwrap();
            let root_text: loro::LoroText = root_meta.ensure_mergeable_text("content_raw").unwrap();
            root_text.insert(0, "Shared steps").unwrap();

            let child = tree.create(Some(root)).unwrap();
            let child_meta = tree.get_meta(child).unwrap();
            child_meta
                .insert(STABLE_ID, loro::LoroValue::from("shared-child"))
                .unwrap();
            let child_text: loro::LoroText =
                child_meta.ensure_mergeable_text("content_raw").unwrap();
            child_text.insert(0, "Fold gently").unwrap();
            shared.commit();
        }

        {
            let global = backend.global_doc().await.unwrap();
            // ALLOW(loro_doc_escape): single-threaded test setup; no concurrent writer
            // exists to observe.
            let doc = global.doc();
            let tree = doc.get_tree(TREE_NAME);
            let node =
                shared_tree::create_mount_node(&tree, None, "stid-descendants", TreeID::new(0, 0))
                    .unwrap();
            set_stable_id(&doc, node, mount).unwrap();
            shared_tree::record_mount(&tree, node, &ShareKind::Block, MountRole::Recipient)
                .unwrap();
            doc.commit();
        }
        backend
            .project_descendants_to_sql(&shared, "stid-descendants")
            .await
            .expect("the descendant projection must land");

        let root_uri = EntityUri::block("shared-root");
        assert!(
            sql.get(root_uri.as_str()).is_some(),
            "the shared root must land under the mount"
        );
        assert!(
            documents.refusal_for_block(&root_uri).is_some(),
            "{root_uri} was projected under {mount}, which is bound to a read-only-homed \
             document, and earns no refusal — it is editable and unwritable at once"
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

    /// The prune-delete `share_subtree` publishes must reach SQL before the
    /// shared descendants are re-projected, or the delete re-removes the rows
    /// just re-created. The only evidence that it landed is the flush's
    /// `ProjectionPass`, so an incomplete one must abort the share rather than
    /// continue into the race.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn share_subtree_fails_when_the_prune_delete_did_not_reach_sql() {
        let (backend, _sql, _dir) = make_backend_with_sql_and_projection(Some(Arc::new(
            FixedPassProjection(holon_core::ProjectionPass::Incomplete { withheld: 2 }),
        )));
        seed_block(&backend, "shared-parent", None, "Shared heading").await;
        seed_block(
            &backend,
            "shared-child",
            Some("shared-parent"),
            "Shared child",
        )
        .await;

        let e = backend
            .share_subtree("block:shared-parent", "none".into())
            .await
            .expect_err("an incomplete flush must abort the share");
        let msg = format!("{e:#}");
        assert!(
            msg.contains("withheld 2") && msg.contains("prune-delete"),
            "the error must name the owed op count and what did not land: {msg}"
        );

        backend.advertiser.close_all().await;
    }

    /// D198.a (page share, sharer side): P keeps its OWN row — same id, title
    /// and Page tag, hung where the mount sits (under `root-page`) — and P's
    /// children keep P as their parent. The mount is a placement record, so
    /// no row carries its id.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn share_page_keeps_the_pages_identity() {
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

        assert_page_row(
            &sql,
            "block:shared-page",
            "My Shared Page",
            "block:root-page",
        );
        assert!(
            sql.get(&mount_id).is_none(),
            "the mount {mount_id} is a placement record and must never project as a row"
        );
        let child = sql
            .get("block:p-child")
            .expect("P's child must be projected");
        assert_eq!(
            child.get("parent_id").and_then(|v| v.as_string()),
            Some("block:shared-page"),
            "P's children keep P as their parent"
        );

        backend.advertiser.close_all().await;
    }

    /// The row a page share projects for its page: P's own id, title and Page
    /// tag, placed under `parent`.
    fn assert_page_row(sql: &RecordingSqlOps, id: &str, title: &str, parent: &str) {
        let row = sql
            .get(id)
            .unwrap_or_else(|| panic!("the shared page {id} must keep its own row"));
        assert_eq!(row.get("content").and_then(|v| v.as_string()), Some(title));
        assert_eq!(
            row.get("parent_id").and_then(|v| v.as_string()),
            Some(parent),
            "the page hangs where its mount sits"
        );
        assert!(
            matches!(row.get("tags"), Some(Value::Array(tags))
                if tags.iter().any(|t| t.as_string() == Some(holon_api::block::PAGE_TAG))),
            "the shared page stays a Page; got {:?}",
            row.get("tags")
        );
    }

    /// D198.a (page share, acceptor side), driven through the real iroh
    /// round-trip: the acceptor projects P under its OWN id, placed where it
    /// accepted it. A second accept of the same share is refused, naming the
    /// page it already places. Moving P on the acceptor moves the
    /// acceptor's mount and nothing in the shared doc.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn accept_page_keeps_the_pages_identity_and_places_it_locally() {
        use holon_api::repository::CoreOperations;

        use crate::loro_backend::LoroBackend;
        use crate::shared_tree::SharedTreeStore;

        let (backend_a, _dir_a) = make_backend();
        let (backend_b, sql_b, _dir_b) = make_backend_with_sql();

        seed_page(&backend_a, "root-a", None, "Root A").await;
        seed_page(&backend_a, "shared-page", Some("root-a"), "My Shared Page").await;
        seed_block(&backend_a, "p-child", Some("shared-page"), "Child under P").await;
        seed_page(&backend_b, "root-b", None, "Root B").await;
        seed_page(&backend_b, "shelf-b", None, "Shelf B").await;
        // The page's `#+TODO` vocabulary, as org ingest stores it.
        {
            let global = backend_a.global_doc().await.unwrap();
            // ALLOW(loro_doc_escape): single-threaded test setup; no concurrent writer
            // exists to observe.
            let doc = global.doc();
            let tid = find_tree_id_by_stable_id(&doc, &EntityUri::block("shared-page")).unwrap();
            doc.get_tree(crate::loro_backend::TREE_NAME)
                .get_meta(tid)
                .unwrap()
                .insert(
                    "properties",
                    loro::LoroValue::from(r#"{"todo_keywords":"TODO DOING | DONE CANCELLED"}"#),
                )
                .unwrap();
            doc.commit();
        }

        let resp = backend_a
            .share_subtree("block:shared-page", "none".into())
            .await
            .unwrap();
        let tj: serde_json::Value = match resp.response.unwrap() {
            Value::String(s) => serde_json::from_str(&s).unwrap(),
            o => panic!("unexpected: {o:?}"),
        };
        let ticket = tj["ticket"].as_str().unwrap().to_string();
        let shared_tree_id = tj["shared_tree_id"].as_str().unwrap().to_string();

        let accept_resp = backend_b
            .accept_shared_subtree("block:root-b", ticket.clone())
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

        assert_page_row(
            &sql_b,
            "block:shared-page",
            "My Shared Page",
            "block:root-b",
        );
        assert!(
            sql_b.get(&mount_id).is_none(),
            "the acceptor's mount {mount_id} must never project as a row"
        );
        let child = sql_b.get("block:p-child").expect("P's child on acceptor");
        assert_eq!(
            child.get("parent_id").and_then(|v| v.as_string()),
            Some("block:shared-page"),
            "P's children keep P as their parent on the acceptor"
        );
        assert_eq!(
            sql_b
                .get("block:shared-page")
                .and_then(|row| row.get("todo_keywords").cloned()),
            Some(Value::String("TODO DOING | DONE CANCELLED".into())),
            "the page's `#+TODO` vocabulary travels with the page"
        );

        let refusal = backend_b
            .accept_shared_subtree("block:shelf-b", ticket)
            .await
            .expect_err("a second accept of a mounted share must be refused");
        assert!(
            format!("{refusal:#}").contains("block:shared-page"),
            "the refusal must name the share by its page, which the user knows: {refusal:#}"
        );

        let b_global = backend_b.global_doc().await.unwrap();
        let authority = LoroBackend::from_document(b_global.clone())
            .with_shared_trees(backend_b.manager.clone() as Arc<dyn SharedTreeStore>);
        assert_eq!(
            authority
                .get_block("block:shared-page")
                .await
                .unwrap()
                .parent_id,
            EntityUri::block("root-b"),
            "the authority answers P's parent from the mount, as SQL does"
        );
        let shared_before = backend_b
            .manager
            .get_doc(&shared_tree_id)
            .unwrap()
            .oplog_vv();
        authority
            .move_block(
                &EntityUri::block("shared-page"),
                EntityUri::block("shelf-b"),
                None,
            )
            .await
            .expect("moving a placed page moves its mount");
        assert_eq!(
            authority
                .get_block("block:shared-page")
                .await
                .unwrap()
                .parent_id,
            EntityUri::block("shelf-b"),
        );
        assert_eq!(
            backend_b
                .manager
                .get_doc(&shared_tree_id)
                .unwrap()
                .oplog_vv(),
            shared_before,
            "a recipient move of the page writes its own mount, never the shared doc"
        );
        backend_b
            .wait_for_workers_idle(SettleScope::LocalWrites)
            .await;
        assert_page_row(
            &sql_b,
            "block:shared-page",
            "My Shared Page",
            "block:shelf-b",
        );

        backend_a.advertiser.close_all().await;
        backend_b.advertiser.close_all().await;
    }

    /// A page share A → B: A shares page `shared-page` (child `p-child`) from
    /// under `root-a`. B holds pages `root-b` and `shelf-b`, and a note under
    /// `shelf-b`.
    struct PageShareFixture {
        a: Arc<LoroShareBackend>,
        b: Arc<LoroShareBackend>,
        sql_b: Arc<RecordingSqlOps>,
        ticket: String,
        shared_tree_id: String,
        _dirs: (TempDir, TempDir),
    }

    impl PageShareFixture {
        async fn shared() -> Self {
            let (a, dir_a) = make_backend();
            let (b, sql_b, dir_b) = make_backend_with_sql();
            seed_page(&a, "root-a", None, "Root A").await;
            seed_page(&a, "shared-page", Some("root-a"), "My Shared Page").await;
            seed_block(&a, "p-child", Some("shared-page"), "Child under P").await;
            seed_page(&b, "root-b", None, "Root B").await;
            seed_page(&b, "shelf-b", None, "Shelf B").await;
            seed_block(&b, "note-b", Some("shelf-b"), "A note of B's").await;
            let resp = a
                .share_subtree("block:shared-page", "none".into())
                .await
                .unwrap();
            let json: serde_json::Value = match resp.response.unwrap() {
                Value::String(s) => serde_json::from_str(&s).unwrap(),
                o => panic!("unexpected: {o:?}"),
            };
            Self {
                a,
                b,
                sql_b,
                ticket: json["ticket"].as_str().unwrap().to_string(),
                shared_tree_id: json["shared_tree_id"].as_str().unwrap().to_string(),
                _dirs: (dir_a, dir_b),
            }
        }

        async fn accepted() -> Self {
            let fixture = Self::shared().await;
            fixture
                .b
                .accept_shared_subtree("block:root-b", fixture.ticket.clone())
                .await
                .unwrap();
            fixture
        }

        /// B's Loro authority, following B's mounts into B's shared docs.
        async fn authority_b(&self) -> crate::loro_backend::LoroBackend {
            use crate::shared_tree::SharedTreeStore;
            crate::loro_backend::LoroBackend::from_document(self.b.global_doc().await.unwrap())
                .with_shared_trees(self.b.manager.clone() as Arc<dyn SharedTreeStore>)
        }

        /// B's cell registry, the route production structural deletes take.
        async fn registry_b(&self) -> crate::block_cell_registry::BlockCellRegistry {
            use crate::shared_tree::SharedTreeStore;
            let store = self.b.store.read().await;
            crate::block_cell_registry::BlockCellRegistry::with_loro(
                store.get_doc(DocScope::Global).await.unwrap(),
                store.get_doc(DocScope::Layout).await.unwrap(),
            )
            .with_shared_trees(self.b.manager.clone() as Arc<dyn SharedTreeStore>)
        }

        /// B no longer holds the share: no mount, no loaded doc, no rows.
        async fn assert_b_left_the_share(&self) {
            assert_eq!(self.mounts_on_b().await, 0, "the placement is gone");
            assert!(
                self.b.manager.get_doc(&self.shared_tree_id).is_none(),
                "B left the share: its copy of the shared doc is unloaded"
            );
            assert!(self.sql_b.get("block:shared-page").is_none());
            assert!(self.sql_b.get("block:p-child").is_none());
            assert!(
                self.b.degraded_bus().current().iter().any(|c| {
                    c.subject == "block:shared-page"
                        && c.reason.condition_kind() == "left-shared-page"
                }),
                "leaving is disclosed: {:?}",
                self.b.degraded_bus().current()
            );
            let owner_doc = self.a.manager.get_doc(&self.shared_tree_id).unwrap();
            assert_eq!(
                owner_doc
                    .get_tree(crate::loro_backend::TREE_NAME)
                    .roots()
                    .len(),
                1,
                "the owner's page is unchanged"
            );
        }

        /// How many live nodes of B's global tree carry stable id `id`.
        async fn global_nodes_named(&self, id: &str) -> usize {
            self.b
                .global_doc()
                .await
                .unwrap()
                .with_read(|doc| {
                    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                    Ok(tree
                        .get_nodes(false)
                        .into_iter()
                        .filter(|n| {
                            !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist)
                        })
                        .filter(|n| read_stable_id(&tree, n.id).as_deref() == Some(id))
                        .count())
                })
                .unwrap()
        }

        /// How many live mounts of the share B's global tree holds.
        async fn mounts_on_b(&self) -> usize {
            self.b
                .global_doc()
                .await
                .unwrap()
                .with_read(|doc| {
                    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                    Ok(tree
                        .get_nodes(false)
                        .into_iter()
                        .filter(|n| {
                            !matches!(n.parent, TreeParentId::Deleted | TreeParentId::Unexist)
                        })
                        .filter(|n| {
                            shared_tree::read_mount_info(&tree, n.id)
                                .is_some_and(|m| m.shared_tree_id == self.shared_tree_id)
                        })
                        .count())
                })
                .unwrap()
        }

        fn sql_parent_on_b(&self, id: &str) -> Option<String> {
            self.sql_b.get(id).and_then(|row| {
                row.get("parent_id")
                    .and_then(|v| v.as_string().map(str::to_string))
            })
        }

        async fn close(self) {
            self.a.advertiser.close_all().await;
            self.b.advertiser.close_all().await;
        }
    }

    /// R5: the one-placement check holds under a race. Exactly one of two
    /// concurrent accepts places the share, and the other leaves nothing
    /// behind: no minted peer id, and no doc of its own bound to the
    /// advertiser.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn two_concurrent_accepts_of_one_ticket_place_the_share_once() {
        let fixture = PageShareFixture::shared().await;
        let barrier = Arc::new(tokio::sync::Barrier::new(2));
        let accept = |parent: &'static str| {
            let b = fixture.b.clone();
            let ticket = fixture.ticket.clone();
            let barrier = barrier.clone();
            tokio::spawn(async move {
                barrier.wait().await;
                b.accept_shared_subtree(parent, ticket)
                    .await
                    .map(|_| ())
                    .map_err(|e| format!("{e:#}"))
            })
        };
        let first = accept("block:root-b");
        let second = accept("block:shelf-b");
        let outcomes = [first.await.unwrap(), second.await.unwrap()];

        assert_eq!(
            outcomes.iter().filter(|o| o.is_ok()).count(),
            1,
            "exactly one of two concurrent accepts of one ticket may place the share: \
             {outcomes:?}"
        );
        assert!(
            outcomes.iter().any(|o| o.as_ref().is_err_and(|e| {
                e.contains("already accepted") || e.contains("already being accepted")
            })),
            "the losing accept is refused as a second placement: {outcomes:?}"
        );
        assert_eq!(fixture.mounts_on_b().await, 1, "one share, one mount");
        assert_eq!(
            fixture
                .b
                .snapshot_store
                .load_generation(&fixture.shared_tree_id)
                .unwrap(),
            1,
            "only the winning accept minted a peer id for the share"
        );

        // The owner's next edit, pushed to B's advertiser, lands in the doc B
        // registered: the loser bound none of its own.
        {
            let doc = fixture.a.manager.get_doc(&fixture.shared_tree_id).unwrap();
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            tree.get_meta(tree.roots()[0])
                .unwrap()
                .insert("r5-probe", "pushed by the owner")
                .unwrap();
            doc.commit();
        }
        fixture
            .a
            .sync_with_peers(&fixture.shared_tree_id)
            .await
            .expect("the owner pushes its edit to B");
        let registered = fixture.b.manager.get_doc(&fixture.shared_tree_id).unwrap();
        let tree = registered.get_tree(crate::loro_backend::TREE_NAME);
        assert!(
            tree.get_meta(tree.roots()[0])
                .unwrap()
                .get("r5-probe")
                .is_some(),
            "the owner's push reached the doc B registered for the share"
        );
        fixture.close().await;
    }

    /// R4: a share made before share kinds were recorded carries no record in
    /// its doc. Accepting it is refused before anything is placed, and the
    /// refusal says why and what the owner does about it.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn accepting_a_share_without_a_kind_record_names_the_remedy() {
        let fixture = PageShareFixture::shared().await;
        let doc = fixture.a.manager.get_doc(&fixture.shared_tree_id).unwrap();
        shared_tree::erase_share_record(&doc).unwrap();

        let refused = fixture
            .b
            .accept_shared_subtree("block:root-b", fixture.ticket.clone())
            .await
            .map(|_| ())
            .expect_err("a share with no kind record is not placed");
        let message = format!("{refused:#}");
        assert!(
            message.contains("carries no share record")
                && message.contains("Ask the owner to unshare it and share it again"),
            "the refusal names the missing record and the remedy: {message}"
        );
        assert_eq!(fixture.mounts_on_b().await, 0, "nothing is placed");
        fixture.close().await;
    }

    /// R4: a mount written before kinds were recorded gets its kind once, from
    /// the sharer's record when the shared doc has one, else from the root's
    /// `Page` tag.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn an_unrecorded_mount_kind_is_recorded_from_the_share_or_the_root_tag() {
        let fixture = PageShareFixture::accepted().await;
        let id = fixture.shared_tree_id.clone();
        let mount_kind = || async {
            fixture
                .b
                .global_doc()
                .await
                .unwrap()
                .with_read(|doc| {
                    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                    let mount = shared_tree::find_mount_node(&tree, &id).unwrap();
                    Ok(shared_tree::read_mount_info(&tree, mount).unwrap().kind)
                })
                .unwrap()
        };
        let erase_mount_kind = || async {
            fixture
                .b
                .global_doc()
                .await
                .unwrap()
                .with_write(WriteOrigin::ShareLifecycle, |txn| {
                    let tree = txn.doc().get_tree(crate::loro_backend::TREE_NAME);
                    let mount = shared_tree::find_mount_node(&tree, &id).unwrap();
                    shared_tree::erase_kind(&tree.get_meta(mount)?)?;
                    txn.commit();
                    Ok(())
                })
                .unwrap()
        };
        let page = shared_tree::ShareKind::Page {
            root: "shared-page".to_string(),
        };
        let shared_doc = fixture.b.manager.get_doc(&id).unwrap();

        erase_mount_kind().await;
        assert_eq!(mount_kind().await, shared_tree::KindRecord::Unrecorded);
        let from_record = fixture
            .b
            .record_legacy_share_kind(&id, &shared_doc)
            .await
            .unwrap();
        assert_eq!(from_record, page, "the sharer's record decides");
        assert_eq!(
            mount_kind().await,
            shared_tree::KindRecord::Recorded(page.clone())
        );

        erase_mount_kind().await;
        shared_tree::erase_share_record(&shared_doc).unwrap();
        let from_tag = fixture
            .b
            .record_legacy_share_kind(&id, &shared_doc)
            .await
            .unwrap();
        assert_eq!(
            from_tag, page,
            "with no record, the root's Page tag decides"
        );
        assert_eq!(mount_kind().await, shared_tree::KindRecord::Recorded(page));
        fixture.close().await;
    }

    /// R4: a share's kind is fixed when it is made. The owner dropping the
    /// `Page` tag from the shared page afterwards changes a field of the page,
    /// never where the recipient placed it: the authority and SQL keep
    /// agreeing, and the recipient's moves keep moving its own mount.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn the_owner_dropping_the_page_tag_leaves_the_recipients_placement_intact() {
        use holon_api::repository::CoreOperations;

        let fixture = PageShareFixture::accepted().await;
        {
            let doc = fixture.a.manager.get_doc(&fixture.shared_tree_id).unwrap();
            let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
            let root = tree.roots()[0];
            tree.get_meta(root)
                .unwrap()
                .insert("tags", loro::LoroValue::from("[]"))
                .unwrap();
            doc.commit();
        }
        fixture
            .b
            .sync_with_peers(&fixture.shared_tree_id)
            .await
            .expect("B pulls the owner's edit");
        fixture
            .b
            .wait_for_workers_idle(SettleScope::LocalWrites)
            .await;
        let authority = fixture.authority_b().await;
        assert!(
            !authority
                .get_block("block:shared-page")
                .await
                .unwrap()
                .is_page(),
            "precondition: the owner's untagging reached B"
        );

        for parent in ["root-b", "shelf-b"] {
            if parent == "shelf-b" {
                authority
                    .move_block(
                        &EntityUri::block("shared-page"),
                        EntityUri::block("shelf-b"),
                        None,
                    )
                    .await
                    .expect("the recipient's move of its placed page still moves its mount");
                fixture
                    .b
                    .wait_for_workers_idle(SettleScope::LocalWrites)
                    .await;
            }
            assert_eq!(
                authority
                    .get_block("block:shared-page")
                    .await
                    .unwrap()
                    .parent_id,
                EntityUri::block(parent),
                "the authority places the page under B's {parent}"
            );
            assert_eq!(
                fixture.sql_parent_on_b("block:shared-page").as_deref(),
                Some(format!("block:{parent}").as_str()),
                "SQL agrees with the authority on where B placed the page"
            );
        }
        fixture.close().await;
    }

    /// R2: the handle of a page share is the page. `unshare` of the page's own
    /// id tears the share down on this device; the mount id is internal.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn unshare_takes_the_shared_page_as_its_handle() {
        let fixture = PageShareFixture::accepted().await;
        fixture
            .b
            .unshare("block:shared-page")
            .await
            .expect("unshare names a page share by its page");
        assert_eq!(fixture.mounts_on_b().await, 0);
        assert!(fixture.sql_b.get("block:shared-page").is_none());
        assert!(fixture.sql_b.get("block:p-child").is_none());
        fixture.close().await;
    }

    /// R7: a move anchored AFTER a placed page lands right after it among the
    /// recipient's blocks — the anchor is the page's mount, which is where the
    /// page sits in the recipient's tree.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_move_anchored_after_a_placed_page_lands_after_its_mount() {
        use holon_api::repository::CoreOperations;

        let fixture = PageShareFixture::accepted().await;
        seed_block(&fixture.b, "first-b", Some("root-b"), "B's first block").await;
        let authority = fixture.authority_b().await;
        authority
            .move_block(
                &EntityUri::block("first-b"),
                EntityUri::block("root-b"),
                None,
            )
            .await
            .unwrap();
        authority
            .move_block(
                &EntityUri::block("note-b"),
                EntityUri::block("root-b"),
                Some(EntityUri::block("shared-page")),
            )
            .await
            .expect("a move anchored after the placed page");
        fixture
            .b
            .wait_for_workers_idle(SettleScope::LocalWrites)
            .await;

        let children = authority.list_children("block:root-b").await.unwrap();
        let at = |id: &str| {
            children
                .iter()
                .position(|c| c.as_str() == id)
                .unwrap_or_else(|| panic!("{id} is not under root-b: {children:?}"))
        };
        assert_eq!(
            at("block:note-b"),
            at("block:shared-page") + 1,
            "note-b sits right after the placed page: {children:?}"
        );
        fixture.close().await;
    }

    /// R1: a recipient's leave of a placed page removes only its own
    /// placement, and a plain block delete refuses it. It leaves the share on
    /// this device — the mount, the shared doc and every projected row go —
    /// with a notice, and never writes the shared doc, so the owner's page
    /// is unchanged.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_recipient_delete_of_the_placed_page_leaves_the_share() {
        use holon_api::repository::CoreOperations;

        let fixture = PageShareFixture::accepted().await;
        let shared_doc_b = fixture.b.manager.get_doc(&fixture.shared_tree_id).unwrap();
        let untouched = shared_doc_b.oplog_vv();

        let authority = fixture.authority_b().await;
        let refused = authority
            .delete_block("block:shared-page")
            .await
            .expect_err("a plain block delete never removes a received page");
        assert!(
            refused
                .to_string()
                .contains("delete block:shared-page to leave the share"),
            "the refusal names the way out: {refused}"
        );
        assert_eq!(
            fixture.mounts_on_b().await,
            1,
            "the refused delete changed nothing"
        );
        assert_eq!(
            authority
                .leave_received_pages("block:shared-page")
                .await
                .expect("a recipient may leave a placed page's share"),
            vec![EntityUri::block("shared-page")]
        );

        assert_eq!(
            shared_doc_b.oplog_vv(),
            untouched,
            "the recipient's delete never writes the shared doc"
        );
        fixture.assert_b_left_the_share().await;
        fixture.close().await;
    }

    /// Deleting the block a received page hangs under removes the page from
    /// this device, so it leaves the page's share through the same exit as a
    /// delete of the page, on the registry route production deletes take.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn deleting_the_block_a_received_page_hangs_under_leaves_its_share() {
        use holon_core::cell_registry::EntityCellRegistry;

        let fixture = PageShareFixture::accepted().await;
        let registry = fixture.registry_b().await;
        let parent = EntityUri::block("root-b");
        assert!(
            registry.leave_received_shares(&parent).await.unwrap(),
            "root-b holds a received page, so deleting it leaves that page's share"
        );
        assert!(registry.delete_entity(&parent).await.unwrap());

        fixture.assert_b_left_the_share().await;
        fixture.close().await;
    }

    /// A create that names a received page's id — an ingest of a file
    /// carrying `:ID: <P>` — reconciles the placed page and never mints a
    /// second node for it, on the single and the batched ingest seam.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_create_naming_a_received_page_mints_no_second_node() {
        use holon_core::cell_registry::EntityCellRegistry;

        let fixture = PageShareFixture::accepted().await;
        let registry = fixture.registry_b().await;
        let page = EntityUri::block("shared-page");
        assert_eq!(fixture.global_nodes_named(page.id()).await, 0);
        let request = holon_core::block_ordering::BlockCreateRequest {
            parent_id: EntityUri::block("shelf-b"),
            id: page.clone(),
            content: holon_api::BlockContent::text("My Shared Page"),
            properties: HashMap::new(),
            edges: holon_api::BlockEdges::default(),
        };
        registry
            .create_entity(
                &request.parent_id,
                None,
                &page,
                request.content.clone(),
                &request.properties,
                &request.edges,
            )
            .await
            .unwrap();
        assert_eq!(
            fixture.global_nodes_named(page.id()).await,
            0,
            "the single-block ingest seam minted a second node for the placed page"
        );
        registry
            .create_entities(std::slice::from_ref(&request))
            .await
            .unwrap();
        assert_eq!(
            fixture.global_nodes_named(page.id()).await,
            0,
            "the batched ingest seam minted a second node for the placed page"
        );
        assert_eq!(fixture.mounts_on_b().await, 1, "the page is still placed");
        fixture.close().await;
    }

    /// A plain block delete of the block a received page hangs under is
    /// refused, as a plain delete of the page is: only the share exit may take
    /// the page off this device.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_plain_delete_of_the_block_a_received_page_hangs_under_is_refused() {
        use holon_api::repository::CoreOperations;

        let fixture = PageShareFixture::accepted().await;
        let authority = fixture.authority_b().await;
        let refused = authority
            .delete_block("block:root-b")
            .await
            .expect_err("a plain block delete never removes a received page");
        assert!(
            refused.to_string().contains("block:shared-page"),
            "the refusal names the received page: {refused}"
        );
        let refused = authority
            .delete_blocks(vec!["block:root-b".to_string()])
            .await
            .expect_err("a batch block delete never removes a received page");
        assert!(
            refused.to_string().contains("block:shared-page"),
            "the refusal names the received page: {refused}"
        );
        assert_eq!(
            fixture.mounts_on_b().await,
            1,
            "the refused deletes changed nothing"
        );
        assert!(fixture.b.manager.get_doc(&fixture.shared_tree_id).is_some());
        fixture.close().await;
    }

    /// R1: a batch that left a share cannot be rolled back. Its leave is
    /// irreversible, and the whole-document rewind would restore a mount whose
    /// share is gone, so the rollback refuses before reverting anything.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_batch_that_left_a_share_refuses_its_rollback() {
        use holon_core::BatchWindow;
        use holon_core::batch_rollback::BatchRollback;
        use holon_core::batch_rollback::RollbackRefused;
        use holon_core::traits::CrudOperations;

        let fixture = PageShareFixture::accepted().await;
        let ops = crate::LoroBlockOperations::new(fixture.b.store.clone()).with_shared_trees(
            fixture.b.manager.clone() as Arc<dyn crate::shared_tree::SharedTreeStore>,
        );
        let mut window = BatchWindow::opened(ops.observe().await.unwrap());
        ops.delete("block:shared-page")
            .await
            .expect("the batch's delete leaves the share");
        window.absorb(ops.observe().await.unwrap());
        assert_eq!(
            fixture.mounts_on_b().await,
            0,
            "the leave removed the mount"
        );

        let refused = ops
            .rollback_to(&window)
            .await
            .expect_err("a rollback across a share exit is refused");
        assert!(
            matches!(&refused, RollbackRefused::Unreachable { doc, .. }
                if *doc == format!("shared:{}", fixture.shared_tree_id)),
            "the refusal names the share that left: {refused:?}"
        );
        assert_eq!(
            fixture.mounts_on_b().await,
            0,
            "the refused rollback restored no mount"
        );
        fixture.close().await;
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
            // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
            // exists to observe.
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
            // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
            // exists to observe.
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
        // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
        // exists to observe.
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
            // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
            // exists to observe.
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
        // ALLOW(loro_doc_escape): single-threaded test assertion; no concurrent writer
        // exists to observe.
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

    /// `(mount_block_id, shared_tree_id)` of a fresh share of `id`.
    async fn share_ok(b: &LoroShareBackend, id: &str) -> (String, String) {
        let r = b
            .share_subtree(id, "none".into())
            .await
            .unwrap_or_else(|e| panic!("share {id}: {e}"));
        let j: serde_json::Value = match r.response {
            Some(Value::String(s)) => serde_json::from_str(&s).unwrap(),
            other => panic!("share {id} response: {other:?}"),
        };
        (
            j["mount_block_id"].as_str().unwrap().to_string(),
            j["shared_tree_id"].as_str().unwrap().to_string(),
        )
    }

    fn reads_of(
        b: &LoroShareBackend,
        global: Arc<crate::loro_document::LoroDocument>,
    ) -> crate::loro_backend::LoroBackend {
        crate::loro_backend::LoroBackend::from_document(global)
            .with_shared_trees(b.manager.clone() as Arc<dyn crate::shared_tree::SharedTreeStore>)
    }

    fn page_id(answer: holon_core::OwningPage) -> String {
        match answer {
            holon_core::OwningPage::Page(p) => p.block.id.to_string(),
            other => panic!("expected a page, got {other:?}"),
        }
    }

    /// ADR 0028 A7: a share whose subtree holds a mount, or that sits inside a
    /// shared subtree, is refused with a typed error and changes nothing.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn share_subtree_refuses_nested_shares() {
        let (b, _d) = make_backend();
        seed_page(&b, "host", None, "Host").await;
        seed_block(&b, "gamma", Some("host"), "Gamma").await;
        seed_block(&b, "gamma-one", Some("gamma"), "Gamma one").await;
        let (m1, st1) = share_ok(&b, "block:gamma").await;

        let outer = b
            .share_subtree("block:host", "none".into())
            .await
            .expect_err("sharing a page that holds a mount must be refused");
        assert_eq!(
            outer.downcast_ref::<NestedShareRefusal>(),
            Some(&NestedShareRefusal::ContainsShare {
                id: "block:host".into(),
                mount: m1.clone(),
            }),
            "{outer}"
        );
        let inner = b
            .share_subtree("block:gamma-one", "none".into())
            .await
            .expect_err("sharing a block inside a share must be refused");
        assert_eq!(
            inner.downcast_ref::<NestedShareRefusal>(),
            Some(&NestedShareRefusal::InsideShare {
                id: "block:gamma-one".into(),
                shared_tree_id: st1,
            }),
            "{inner}"
        );

        let reads = reads_of(&b, b.global_doc().await.unwrap());
        for id in ["block:gamma", "block:gamma-one", m1.as_str()] {
            assert_eq!(
                page_id(reads.owning_page(id).unwrap()),
                m1,
                "owning_page({id})"
            );
        }
        assert_eq!(
            page_id(reads.owning_page("block:host").unwrap()),
            "block:host"
        );
        b.advertiser.close_all().await;
    }

    /// A share whose mount is gone, or whose doc this device has not loaded,
    /// answers a typed chain break instead of an internal error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn owning_page_types_a_missing_mount_and_an_unloaded_share() {
        use holon_api::repository::CoreOperations;
        use holon_core::ChainBreak;
        use holon_core::OwningPage;
        let (b, _d) = make_backend();
        seed_page(&b, "host", None, "Host").await;
        seed_block(&b, "gamma", Some("host"), "Gamma").await;
        let (m1, st1) = share_ok(&b, "block:gamma").await;

        let unloaded =
            crate::loro_backend::LoroBackend::from_document(b.global_doc().await.unwrap());
        assert_eq!(
            unloaded.owning_page(&m1).unwrap(),
            OwningPage::Broken(ChainBreak::SharedSubtreeNotMaterialized {
                shared_tree_id: st1.clone()
            })
        );

        let reads = reads_of(&b, b.global_doc().await.unwrap());
        assert_eq!(page_id(reads.owning_page("block:gamma").unwrap()), m1);
        reads.delete_block("block:host").await.unwrap();
        assert_eq!(
            reads.owning_page("block:gamma").unwrap(),
            OwningPage::Broken(ChainBreak::OrphanedShare {
                shared_tree_id: st1
            })
        );
        b.advertiser.close_all().await;
    }

    /// ADR 0028 A7 on the recipient: accepting a share under a block that is
    /// already inside a share is refused with the typed error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn accept_refuses_a_parent_inside_a_share() {
        let (a, _da) = make_backend();
        let (b, _db) = make_backend();
        seed_page(&a, "host", None, "Host").await;
        seed_block(&a, "gamma", Some("host"), "Gamma").await;
        seed_page(&b, "root-b", None, "Root B").await;
        seed_block(&b, "bx", Some("root-b"), "bx").await;
        seed_block(&b, "bx1", Some("bx"), "bx1").await;
        let (_, st_b) = share_ok(&b, "block:bx").await;
        let r = a.share_subtree("block:gamma", "none".into()).await.unwrap();
        let j: serde_json::Value = match r.response {
            Some(Value::String(s)) => serde_json::from_str(&s).unwrap(),
            other => panic!("share response: {other:?}"),
        };
        let ticket = j["ticket"].as_str().unwrap().to_string();

        let refused = b
            .accept_shared_subtree("block:bx1", ticket)
            .await
            .expect_err("accepting under a block inside a share must be refused");
        assert_eq!(
            refused.downcast_ref::<NestedShareRefusal>(),
            Some(&NestedShareRefusal::InsideShare {
                id: "block:bx1".into(),
                shared_tree_id: st_b,
            }),
            "{refused}"
        );
        a.advertiser.close_all().await;
        b.advertiser.close_all().await;
    }

    /// A shared doc saved with another share's mount inside it (before the
    /// nesting refusal existed) is disclosed at rehydrate, not repaired.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn rehydrating_a_nested_share_discloses_it() {
        let (b, keychain, dir) = make_backend_with_keychain();
        seed_page(&b, "host", None, "Host").await;
        seed_block(&b, "gamma", Some("host"), "Gamma").await;
        let (_, st1) = share_ok(&b, "block:gamma").await;
        let shared = b.manager.get_doc(&st1).expect("the share's doc is loaded");
        {
            let tree = shared.get_tree(crate::loro_backend::TREE_NAME);
            let root = tree.roots()[0];
            shared_tree::create_mount_node(&tree, Some(root), "planted-inner-share", root).unwrap();
            shared.commit();
        }
        b.snapshot_store.save(&st1, &shared).unwrap();
        b.advertiser.close_all().await;
        b.flush_all().await;
        drop(shared);
        drop(b);
        tokio::time::sleep(Duration::from_millis(200)).await;

        let b = make_backend_at(dir.path(), test_credentials(keychain));
        let mut changes = b.degraded_bus().subscribe().changes;
        assert_eq!(rehydrate_over(&b).await, 1, "the nested share still loads");
        let mut disclosed = Vec::new();
        while let Ok(change) = changes.try_recv() {
            if let Some(event) = change.raised()
                && let ConditionKind::NestedShareLoaded { .. } = &event.reason
            {
                disclosed.push(event.subject.clone());
            }
        }
        assert_eq!(disclosed, vec![st1]);
        b.advertiser.close_all().await;
    }

    /// An older peer can sync a mount into a shared doc this device has
    /// already loaded; the live import discloses it like a load does.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_nested_share_arriving_by_sync_is_disclosed() {
        let (b, _d) = make_backend();
        seed_page(&b, "host", None, "Host").await;
        seed_block(&b, "gamma", Some("host"), "Gamma").await;
        let (_, st1) = share_ok(&b, "block:gamma").await;
        let shared = b.manager.get_doc(&st1).expect("the share's doc is loaded");
        b.wait_for_workers_idle(SettleScope::LocalWrites).await;
        let mut changes = b.degraded_bus().subscribe().changes;

        let older_peer = shared.fork();
        {
            let tree = older_peer.get_tree(crate::loro_backend::TREE_NAME);
            let root = tree.roots()[0];
            shared_tree::create_mount_node(&tree, Some(root), "synced-inner-share", root).unwrap();
            older_peer.commit();
        }
        shared
            .import(
                &older_peer
                    .export(loro::ExportMode::updates(&shared.oplog_vv()))
                    .unwrap(),
            )
            .unwrap();
        b.wait_for_workers_idle(SettleScope::LocalWrites).await;

        let mut disclosed = Vec::new();
        while let Ok(change) = changes.try_recv() {
            if let Some(event) = change.raised()
                && let ConditionKind::NestedShareLoaded { .. } = &event.reason
            {
                disclosed.push(event.subject.clone());
            }
        }
        assert_eq!(disclosed, vec![st1.clone()]);

        let second_peer = shared.fork();
        {
            let tree = second_peer.get_tree(crate::loro_backend::TREE_NAME);
            let root = tree.roots()[0];
            shared_tree::create_mount_node(&tree, Some(root), "second-inner-share", root).unwrap();
            second_peer.commit();
        }
        shared
            .import(
                &second_peer
                    .export(loro::ExportMode::updates(&shared.oplog_vv()))
                    .unwrap(),
            )
            .unwrap();
        b.wait_for_workers_idle(SettleScope::LocalWrites).await;
        let mut second = Vec::new();
        while let Ok(change) = changes.try_recv() {
            if let Some(event) = change.raised()
                && let ConditionKind::NestedShareLoaded { .. } = &event.reason
            {
                second.push(event.subject.clone());
            }
        }
        assert_eq!(second, vec![st1], "a second nested mount is disclosed too");
        b.advertiser.close_all().await;
    }

    /// Two paired devices that accept one ticket before they sync, each
    /// creating a mount: `b`'s mount `m1`, and `m2` from a device with peer
    /// id `other_peer`, merged into `b`'s global doc after `warm` has cached
    /// `m1`.
    struct DuplicatedShare {
        b: Arc<LoroShareBackend>,
        _dir: TempDir,
        global: Arc<crate::loro_document::LoroDocument>,
        warm: crate::loro_backend::LoroBackend,
        changes: tokio::sync::broadcast::Receiver<holon_api::condition_bus::ConditionChange>,
        m1: String,
        m1_tid: TreeID,
        m2: String,
        m2_tid: TreeID,
    }

    impl DuplicatedShare {
        async fn new(other_peer: u64) -> Self {
            let (b, dir) = make_backend();
            seed_page(&b, "host", None, "Host").await;
            seed_block(&b, "gamma", Some("host"), "Gamma").await;
            let global = b.global_doc().await.unwrap();
            let other_device = global.with_read(|doc| Ok(doc.fork())).unwrap();
            other_device.set_peer_id(other_peer).unwrap();
            let (m1, st1) = share_ok(&b, "block:gamma").await;

            let bus = Arc::new(ConditionBus::new());
            let changes = bus.subscribe().changes;
            let warm = reads_of(&b, global.clone()).with_condition_bus(bus);
            assert_eq!(page_id(warm.owning_page("block:gamma").unwrap()), m1);

            let (m1_tid, m2_tid) = global
                .with_read(|doc| {
                    let tree = doc.get_tree(crate::loro_backend::TREE_NAME);
                    let m1_tid = find_tree_id_by_stable_id(doc, &EntityUri::parse(&m1)?)
                        .expect("m1 is live");
                    let shared_root = shared_tree::read_mount_info(&tree, m1_tid)
                        .expect("m1 is a mount")
                        .shared_root;
                    let host =
                        find_tree_id_by_stable_id(&other_device, &EntityUri::parse("block:host")?)
                            .expect("host is live on the other device");
                    let other_tree = other_device.get_tree(crate::loro_backend::TREE_NAME);
                    let m2_tid =
                        shared_tree::create_mount_node(&other_tree, Some(host), &st1, shared_root)?;
                    set_stable_id(&other_device, m2_tid, "second-mount")?;
                    other_device.commit();
                    Ok((m1_tid, m2_tid))
                })
                .unwrap();
            let update = global
                .with_read(|doc| {
                    Ok(other_device.export(loro::ExportMode::updates(&doc.oplog_vv()))?)
                })
                .unwrap();
            global.apply_update(&update).unwrap();
            Self {
                b,
                _dir: dir,
                global,
                warm,
                changes,
                m1,
                m1_tid,
                m2: "block:second-mount".to_string(),
                m2_tid,
            }
        }

        fn warm(&self) -> String {
            page_id(self.warm.owning_page("block:gamma").unwrap())
        }

        fn cold(&self) -> String {
            page_id(
                reads_of(&self.b, self.global.clone())
                    .owning_page("block:gamma")
                    .unwrap(),
            )
        }

        fn events(&mut self) -> Vec<String> {
            duplicate_mount_events(&mut self.changes)
        }

        fn write(
            &self,
            origin: crate::write_origin::WriteOrigin,
            f: impl FnOnce(&crate::loro_document::WriteTxn) -> anyhow::Result<()>,
        ) {
            self.global
                .with_write(origin, |txn| {
                    f(txn)?;
                    txn.commit();
                    Ok(())
                })
                .unwrap()
        }

        fn delete(&self, node: TreeID) {
            self.write(crate::write_origin::WriteOrigin::BlockOps, |txn| {
                Ok(txn.get_tree(crate::loro_backend::TREE_NAME).delete(node)?)
            });
        }
    }

    /// After the merge a warm and a cold mount cache both answer the mount
    /// with the smallest TreeID, and the condition follows the set of mounts.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_mount_accepted_twice_resolves_to_one_canonical_mount() {
        let mut d = DuplicatedShare::new(1).await;
        assert!(
            d.m2_tid < d.m1_tid,
            "the merged mount sorts first, so the warm m1 is stale"
        );
        let (m1, m2) = (d.m1.clone(), d.m2.clone());

        assert_eq!(d.cold(), m2, "cold after the merge");
        assert_eq!(d.warm(), m2, "warm after the merge");
        assert_eq!(d.warm(), m2, "warm again");
        assert_eq!(d.events(), vec![format!("raised {m2} + [{m1}]")]);

        let m1_tid = d.m1_tid;
        d.write(crate::write_origin::WriteOrigin::BlockOps, |txn| {
            Ok(txn
                .get_tree(crate::loro_backend::TREE_NAME)
                .mov(m1_tid, None)?)
        });
        assert_eq!(d.warm(), m2, "warm after moving the duplicate");
        assert_eq!(
            d.events(),
            Vec::<String>::new(),
            "an unchanged set is disclosed once"
        );

        d.delete(d.m2_tid);
        assert_eq!(d.warm(), m1, "warm after deleting the canonical mount");
        assert_eq!(d.cold(), m1, "cold after deleting the canonical mount");
        assert_eq!(d.events(), vec!["cleared".to_string()]);
        d.b.advertiser.close_all().await;
    }

    /// A batch deletes the canonical mount, a read lands inside the batch, and
    /// the rollback recreates the mount with local ops, which sorts before the
    /// merged one again. Warm and cold agree, and the condition returns.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_rolled_back_mount_delete_is_seen_by_the_warm_cache() {
        let mut d = DuplicatedShare::new(u64::MAX - 1).await;
        assert!(d.m1_tid < d.m2_tid, "the local mount sorts first");
        let (m1, m2) = (d.m1.clone(), d.m2.clone());
        assert_eq!(d.warm(), m1, "warm after the merge");
        assert_eq!(d.events(), vec![format!("raised {m1} + [{m2}]")]);

        let before_batch = d.global.with_read(|doc| Ok(doc.oplog_frontiers())).unwrap();
        d.delete(d.m1_tid);
        assert_eq!(d.warm(), m2, "warm inside the batch");
        assert_eq!(d.events(), vec!["cleared".to_string()]);
        d.write(crate::write_origin::WriteOrigin::BatchRollback, |txn| {
            Ok(txn.revert_to(&before_batch)?)
        });
        assert_eq!(d.cold(), m1, "cold after the rollback");
        assert_eq!(d.warm(), m1, "warm after the rollback");
        assert_eq!(d.events(), vec![format!("raised {m1} + [{m2}]")]);

        d.delete(d.m2_tid);
        assert_eq!(d.warm(), m1, "warm after deleting the duplicate");
        assert_eq!(d.events(), vec!["cleared".to_string()]);
        d.b.advertiser.close_all().await;
    }

    /// A paired device whose ops the global doc imports.
    fn peer_of(d: &DuplicatedShare, peer_id: u64) -> LoroDoc {
        let peer = d.global.with_read(|doc| Ok(doc.fork())).unwrap();
        peer.set_peer_id(peer_id).unwrap();
        peer
    }

    fn import_from(d: &DuplicatedShare, peer: &LoroDoc) {
        let update = d
            .global
            .with_read(|doc| Ok(peer.export(loro::ExportMode::updates(&doc.oplog_vv()))?))
            .unwrap();
        d.global.apply_update(&update).unwrap();
    }

    /// Mount status lives in the node's meta, so a peer can unmount the
    /// canonical mount without a tree op.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_remote_meta_only_unmount_is_seen_by_the_warm_cache() {
        let mut d = DuplicatedShare::new(1).await;
        assert!(d.m2_tid < d.m1_tid);
        assert_eq!(d.warm(), d.m2, "warm before");
        assert_eq!(d.events().len(), 1, "the duplicate is disclosed");
        let peer = peer_of(&d, 7);
        peer.get_tree(crate::loro_backend::TREE_NAME)
            .get_meta(d.m2_tid)
            .unwrap()
            .delete("mount_kind")
            .unwrap();
        peer.commit();
        import_from(&d, &peer);
        assert_eq!(d.cold(), d.m1, "cold after the meta-only unmount");
        assert_eq!(d.warm(), d.m1, "warm after the meta-only unmount");
        assert_eq!(d.warm(), d.m1, "warm again");
        assert_eq!(d.events(), vec!["cleared".to_string()]);
        d.b.advertiser.close_all().await;
    }

    /// A peer turns a plain node with a smaller TreeID into a mount of the
    /// same share by a meta-only edit.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_remote_meta_only_mount_is_seen_by_the_warm_cache() {
        let d = DuplicatedShare::new(1).await;
        assert_eq!(d.warm(), d.m2, "warm before");
        let info = d
            .global
            .with_read(|doc| {
                Ok(shared_tree::read_mount_info(
                    &doc.get_tree(crate::loro_backend::TREE_NAME),
                    d.m2_tid,
                ))
            })
            .unwrap()
            .expect("m2 is a mount");
        let peer = peer_of(&d, 0);
        let host =
            find_tree_id_by_stable_id(&peer, &EntityUri::parse("block:host").unwrap()).unwrap();
        let node = peer
            .get_tree(crate::loro_backend::TREE_NAME)
            .create(Some(host))
            .unwrap();
        set_stable_id(&peer, node, "forged").unwrap();
        peer.commit();
        import_from(&d, &peer);
        assert_eq!(d.warm(), d.m2, "warm after the plain node arrives");

        let meta = peer
            .get_tree(crate::loro_backend::TREE_NAME)
            .get_meta(node)
            .unwrap();
        meta.insert("mount_kind", "shared_tree").unwrap();
        meta.insert("shared_tree_id", info.shared_tree_id.as_str())
            .unwrap();
        meta.insert(
            "shared_root",
            format!("{}:{}", info.shared_root.peer, info.shared_root.counter),
        )
        .unwrap();
        peer.commit();
        import_from(&d, &peer);
        assert!(node < d.m2_tid);
        assert_eq!(d.cold(), "block:forged", "cold after the meta-only mount");
        assert_eq!(d.warm(), "block:forged", "warm after the meta-only mount");
        d.b.advertiser.close_all().await;
    }

    fn duplicate_mount_events(
        changes: &mut tokio::sync::broadcast::Receiver<holon_api::condition_bus::ConditionChange>,
    ) -> Vec<String> {
        use holon_api::condition_bus::ConditionChange;
        let mut events = Vec::new();
        while let Ok(change) = changes.try_recv() {
            match change {
                ConditionChange::Raised(Condition {
                    reason:
                        ConditionKind::DuplicateMount {
                            canonical,
                            duplicates,
                        },
                    ..
                }) => events.push(format!("raised {canonical} + [{}]", duplicates.join(", "))),
                ConditionChange::Cleared(key) if key.kind == ConditionKind::DUPLICATE_MOUNT => {
                    events.push("cleared".to_string())
                }
                _ => {}
            }
        }
        events
    }

    /// A shared id served from the shared-id cache stops resolving once its
    /// doc unloads.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[serial_test::serial]
    async fn a_cached_shared_id_drops_when_its_doc_unloads() {
        use holon_core::OwningPage;
        let (b, _d) = make_backend();
        seed_page(&b, "host", None, "Host").await;
        seed_block(&b, "gamma", Some("host"), "Gamma").await;
        seed_block(&b, "gamma-one", Some("gamma"), "Gamma one").await;
        let (m1, st1) = share_ok(&b, "block:gamma").await;
        let reads = reads_of(&b, b.global_doc().await.unwrap());
        for _ in 0..2 {
            assert_eq!(page_id(reads.owning_page("block:gamma-one").unwrap()), m1);
        }
        b.manager.remove(&st1).expect("the share's doc was loaded");
        assert_eq!(
            reads.owning_page("block:gamma-one").unwrap(),
            OwningPage::Absent
        );
        b.advertiser.close_all().await;
    }
}

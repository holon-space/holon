//! Bidirectional sync between Loro and the command/event bus.
//!
//! From Loro's perspective, the other side of the bridge is just an
//! `OperationProvider` (writes) and an `EventBus` (change notifications).
//! Whatever persistent store is behind them — Turso/SQL today — is an
//! implementation detail `LoroSyncController` does not name.
//!
//! ## Two directions, one loop
//!
//! - **Inbound (EventBus → Loro)**: block events arriving on the event bus
//!   are translated into `LoroBackend` tree mutations. This is the same
//!   logic `LoroReverseSyncAdapter` used to own.
//!
//! - **Outbound (Loro → CommandBus)**: any change to the Loro doc — whether
//!   from a local edit, a peer `doc.import(&delta)`, or an offline
//!   `.loro` file modified by a background sync service — fires
//!   `doc.subscribe_root`, which wakes the controller. The controller then
//!   computes the delta between the last synced frontiers and the current
//!   frontiers, translates it into block ops, and dispatches them via
//!   `OperationProvider::execute_batch_with_origin` tagged `EventOrigin::Loro`.
//!
//! Both directions advance a single `Frontiers` watermark — `last_synced` —
//! persisted in a sidecar file next to the `.loro`. The watermark is the
//! echo-suppression mechanism: after an inbound write applies to Loro,
//! `last_synced` is advanced to `doc.oplog_frontiers()`. The subscription
//! then fires, the outbound pass wakes, sees `current == last`, and is a
//! no-op. No origin tags need to cross the Loro boundary.
//!
//! ## Diff strategy
//!
//! The controller uses `doc.fork_at(&last_synced)` to rewind a copy of the
//! Loro doc to the watermark position, snapshots both the fork (before) and
//! the current doc (after), then diffs the two `HashMap<String, Block>`
//! instances. No persistent block projection is kept in memory — the
//! temporary HashMaps live only inside `on_loro_changed`.
//!
//! `last_synced` is updated ONLY by `on_loro_changed`, never by
//! `on_inbound_event`. This is critical: if the inbound path advanced the
//! watermark, concurrent peer imports would be invisible because
//! `current_frontiers()` includes ALL changes to the doc (inbound + peer).
//! By keeping the watermark in one place, all changes — inbound events,
//! peer imports, background file loads — are guaranteed to appear in the
//! next `fork_at` diff.
//!
//! Inbound-applied blocks re-appear as redundant "creates" in the diff;
//! `SqlOperationProvider` handles these idempotently via `INSERT OR IGNORE`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};

use anyhow::{Context, Result};
use loro::{Frontiers, LoroDoc};
use tokio::sync::{Notify, RwLock};
use tokio_stream::StreamExt as _;
use tracing::{debug, error, info, warn};

use holon_api::block::Block;
use holon_api::types::ContentType;
use holon_api::{BlockContent, EntityName, EntityUri, Value};

use crate::api::{CoreOperations, LoroBackend, snapshot_blocks_from_doc};
use crate::core::datasource::OperationProvider;
use crate::sync::LoroDocumentStore;
use crate::sync::event_bus::{
    AggregateType, Event, EventBus, EventFilter, EventKind, EventOrigin, EventStatus,
};

/// Filename of the sidecar file that persists the sync watermark next to the
/// `.loro` snapshot. One file per `LoroDocumentStore`.
pub const SIDECAR_FILENAME: &str = "holon_tree.loro.sync";

/// Bidirectional sync between Loro and the abstract command/event bus.
pub struct LoroSyncController {
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    command_bus: Arc<dyn OperationProvider>,
    event_bus: Arc<dyn EventBus>,
    sidecar_path: PathBuf,
    /// Frontiers watermark — the doc state after the last successful outbound
    /// reconcile. Updated ONLY by `on_loro_changed`, never by
    /// `on_inbound_event`. This ensures peer imports that land concurrently
    /// with inbound event processing are always captured by the next
    /// `fork_at`-based diff.
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
    pub fn new(
        doc_store: Arc<RwLock<LoroDocumentStore>>,
        command_bus: Arc<dyn OperationProvider>,
        event_bus: Arc<dyn EventBus>,
        storage_dir: PathBuf,
    ) -> Self {
        let sidecar_path = storage_dir.join(SIDECAR_FILENAME);
        let last_synced = load_sidecar_blocking(&sidecar_path);
        Self {
            doc_store,
            command_bus,
            event_bus,
            sidecar_path,
            last_synced: Arc::new(StdMutex::new(last_synced)),
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
    pub async fn start(self) -> Result<LoroSyncControllerHandle> {
        // (1) EventBus subscription — synchronous, before spawn.
        let filter = EventFilter::new()
            .with_aggregate_type(AggregateType::Block)
            .with_status(EventStatus::Confirmed);
        let event_rx = self
            .event_bus
            .subscribe(filter)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to subscribe to EventBus: {}", e))?;

        // (2) Loro subscription — synchronous, before spawn.
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
            let doc = doc_arc.read().await;
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

        // (4) Spawn the main loop.
        let task = tokio::spawn(async move {
            self.run_loop(event_rx).await;
        });

        Ok(LoroSyncControllerHandle {
            _subscription: subscription,
            _task: task,
            last_synced,
            error_count,
            wake,
        })
    }

    async fn run_loop(self, mut event_rx: tokio_stream::wrappers::ReceiverStream<Event>) {
        info!("[LoroSyncController] Started listening to block events");
        loop {
            tokio::select! {
                biased;
                next = event_rx.next() => match next {
                    Some(event) => {
                        let event_id = event.id.clone();
                        if let Err(e) = self.on_inbound_event(&event).await {
                            self.error_count.fetch_add(1, Ordering::SeqCst);
                            error!(
                                "[LoroSyncController] Failed to apply {:?} event for {}: {}",
                                event.event_kind, event.aggregate_id, e
                            );
                        }
                        // Advance the `loro` consumer watermark whether the
                        // event applied successfully or not — failure was
                        // already recorded on `error_count`, and a
                        // permanently-stuck watermark would block test
                        // settlement waits forever. Test invariants
                        // (`inv-loro-no-errors`) catch genuine failures.
                        if let Err(e) = self.event_bus.mark_processed(&event_id, "loro").await {
                            tracing::warn!(
                                "[LoroSyncController] mark_processed(loro, {}) failed: {}",
                                event_id, e
                            );
                        }
                    }
                    None => {
                        info!("[LoroSyncController] EventBus stream closed — exiting loop");
                        return;
                    }
                },
                _ = self.wake.notified() => {
                    if let Err(e) = self.on_loro_changed().await {
                        self.error_count.fetch_add(1, Ordering::SeqCst);
                        error!("[LoroSyncController] Outbound reconcile failed: {}", e);
                    }
                }
            }
        }
    }

    // -- Inbound (EventBus → Loro) -----------------------------------------

    async fn on_inbound_event(&self, event: &Event) -> Result<()> {
        // Echo suppression: skip events we published ourselves when
        // translating the outbound direction.
        if event.origin == EventOrigin::Loro {
            return Ok(());
        }

        let backend = self.get_backend().await?;

        match event.event_kind {
            EventKind::FieldsChanged => {
                if let Some(fields) = event.payload.get("fields") {
                    apply_fields_changed(&backend, &event.aggregate_id, fields).await?;
                }
            }
            EventKind::Deleted => {
                // Delete events may not carry a `data` payload (cascade
                // deletes only include routing metadata). Use aggregate_id.
                let block_id = event
                    .payload
                    .get("data")
                    .and_then(|d| d.get("id"))
                    .and_then(|v| v.as_str())
                    .unwrap_or(&event.aggregate_id);
                apply_delete(&backend, block_id).await?;
            }
            _ => {
                let data = event
                    .payload
                    .get("data")
                    .ok_or_else(|| anyhow::anyhow!("Event payload missing 'data'"))?;
                match event.event_kind {
                    EventKind::Created => apply_create(&backend, data).await?,
                    EventKind::Updated => apply_update_with_backend(&backend, data).await?,
                    EventKind::Deleted | EventKind::FieldsChanged => unreachable!(),
                }
            }
        }

        // Persist the doc. We do NOT update `last_snapshot` here — that is
        // exclusively managed by `on_loro_changed`. This ensures any
        // concurrent Loro changes (peer imports, background file loads) are
        // always captured by the next outbound diff.
        //
        // The inbound apply just mutated Loro, so `subscribe_root` will fire
        // and wake the outbound loop. The outbound diff will see the
        // inbound-applied blocks as "new" and emit redundant creates, which
        // SQL handles idempotently via `INSERT OR IGNORE`.
        {
            let store = self.doc_store.read().await;
            store.save_all().await.context("save_all after inbound")?;
        }

        Ok(())
    }

    // -- Outbound (Loro → CommandBus) --------------------------------------

    async fn on_loro_changed(&self) -> Result<()> {
        let current = self.current_frontiers().await?;
        let last = self.last_synced.lock().unwrap().clone();
        if last == current {
            return Ok(());
        }

        // Read current state.
        let doc_arc = self.raw_doc().await?;
        let after: HashMap<String, Block> = {
            let doc = doc_arc.read().await;
            snapshot_blocks_from_doc(&doc)
        };

        // Fork at the watermark to read the "before" state. fork_at returns
        // an independent LoroDoc rewound to the watermark's position.
        let before: HashMap<String, Block> = {
            let doc = doc_arc.read().await;
            if is_empty_frontiers(&last) {
                HashMap::new()
            } else {
                let fork = doc
                    .fork_at(&last)
                    .context("fork_at watermark for outbound diff")?;
                snapshot_blocks_from_doc(&fork)
            }
        };

        let ops = diff_snapshots_to_ops(&before, &after);

        if !ops.is_empty() {
            debug!(
                "[LoroSyncController] outbound reconcile: before={} after={} ops={}",
                before.len(),
                after.len(),
                ops.len()
            );
            self.command_bus
                .execute_batch_with_origin(&EntityName::new("block"), ops, EventOrigin::Loro)
                .await
                .map_err(|e| anyhow::anyhow!("execute_batch_with_origin failed: {}", e))?;
        }

        // Advance the watermark. This is the ONLY place last_synced is
        // updated — on_inbound_event deliberately does NOT touch it, so
        // concurrent peer imports are always captured by the next diff.
        *self.last_synced.lock().unwrap() = current;
        self.persist_sidecar().await?;
        Ok(())
    }

    // -- Helpers -----------------------------------------------------------

    async fn get_backend(&self) -> Result<LoroBackend> {
        let store = self.doc_store.read().await;
        let collab = store
            .get_global_doc()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get global doc: {}", e))?;
        Ok(LoroBackend::from_document(collab))
    }

    async fn raw_doc(&self) -> Result<Arc<RwLock<LoroDoc>>> {
        let store = self.doc_store.read().await;
        let collab = store
            .get_global_doc()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to get global doc: {}", e))?;
        Ok(collab.doc())
    }

    async fn current_frontiers(&self) -> Result<Frontiers> {
        let doc_arc = self.raw_doc().await?;
        let doc = doc_arc.read().await;
        Ok(doc.oplog_frontiers())
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
        eprintln!(
            "[LORO_DIFF_TRACE] CREATE id={} content={:?}",
            block.id, block.content
        );
        ops.push(("create".to_string(), block_to_params(block)));
    }

    // Updates (in both, but differ).
    //
    // Build a delta params map containing only fields that actually changed
    // between old and new. This prevents Loro from overwriting SQL fields
    // that a concurrent direct write (UI dispatch) has already advanced —
    // if content didn't change in Loro, it simply won't be in the SET clause.
    //
    // When content DID change, embed the Loro "before" content as
    // `_expected_content` so `SqlOperationProvider::prepare_update` can gate
    // the UPDATE on SQL still matching. This prevents a stale outbound
    // reconcile from regressing SQL (Bug #1).
    for (id, new_block) in after {
        if let Some(old_block) = before.get(id) {
            if blocks_differ(old_block, new_block) {
                let mut params = block_diff_params(old_block, new_block);
                if old_block.content != new_block.content {
                    params.insert(
                        "_expected_content".to_string(),
                        Value::String(old_block.content.clone()),
                    );
                }
                if old_block.parent_id != new_block.parent_id {
                    params.insert(
                        "_expected_parent_id".to_string(),
                        Value::String(old_block.parent_id.to_string()),
                    );
                }
                if old_block.marks != new_block.marks {
                    // Stale outbound carrying pre-image marks must not stomp
                    // a fresh local mark edit. Compare via canonical JSON
                    // (`marks_to_json`) since SQL stores marks as a JSON
                    // string. None → empty string sentinel so the WHERE
                    // clause still gates on a stable value.
                    let pre_image = match &old_block.marks {
                        Some(marks) => holon_api::marks_to_json(marks),
                        None => String::new(),
                    };
                    params.insert("_expected_marks".to_string(), Value::String(pre_image));
                }
                eprintln!(
                    "[LORO_DIFF_TRACE] UPDATE id={} content_before={:?} content_after={:?}",
                    id, old_block.content, new_block.content
                );
                ops.push(("update".to_string(), params));
            }
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
        if create_ids.contains(parent_id) {
            if let Some(parent) = all.get(parent_id) {
                visit(parent, all, create_ids, visited, result);
            }
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

pub(crate) fn block_to_params(block: &Block) -> HashMap<String, Value> {
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

    if let Some(ref name) = block.name {
        params.insert("name".to_string(), Value::String(name.clone()));
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
        let header_args = block.get_source_header_args();
        if !header_args.is_empty() {
            if let Ok(json) = serde_json::to_string(&header_args) {
                params.insert("source_header_args".to_string(), Value::String(json));
            }
        }
    }

    // Flatten all raw block properties onto the top-level params map. The
    // downstream `OperationProvider` (e.g. `SqlOperationProvider`) partitions
    // them into SQL columns vs. the `properties` JSON column based on its own
    // `known_columns` table. The Loro side never has to know which fields are
    // first-class columns.
    for (k, v) in &block.properties {
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
    if old.name != new.name {
        if let Some(ref name) = new.name {
            params.insert("name".to_string(), Value::String(name.clone()));
        }
    }
    if old.source_language != new.source_language {
        if let Some(ref lang) = new.source_language {
            params.insert(
                "source_language".to_string(),
                Value::String(lang.to_string()),
            );
        }
    }
    if old.source_name != new.source_name {
        if let Some(ref name) = new.source_name {
            params.insert("source_name".to_string(), Value::String(name.clone()));
        }
    }
    if old.properties_map() != new.properties_map() {
        for (k, v) in &new.properties {
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
        || a.name != b.name
        || a.properties_map() != b.properties_map()
        || a.marks != b.marks
}

// -- Inbound apply helpers (lifted from loro_reverse_sync.rs) --------------

pub(crate) async fn apply_fields_changed(
    backend: &LoroBackend,
    block_id: &str,
    fields: &serde_json::Value,
) -> Result<()> {
    if backend.resolve_to_tree_id(block_id).await.is_none() {
        warn!(
            "[LoroSyncController] Block {} not in Loro during fields_changed — \
             property update will be lost. Indicates a seed/subscribe ordering bug \
             unless this is a document block.",
            block_id
        );
        return Ok(());
    }

    let tuples = fields
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("FieldsChanged payload is not an array"))?;

    let mut props: HashMap<String, Value> = HashMap::new();

    for tuple in tuples {
        let arr = tuple
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("FieldsChanged tuple is not an array"))?;
        let field_name = arr
            .first()
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("FieldsChanged tuple missing field name"))?;
        let new_value = &arr[2];

        if field_name == "content" {
            if let Some(text) = new_value.as_str() {
                let existing = backend
                    .get_block(block_id)
                    .await
                    .map_err(|e| anyhow::anyhow!("get_block({}) failed: {}", block_id, e))?;
                let content = match &existing {
                    b if b.content_type == ContentType::Source => BlockContent::source(
                        b.source_language
                            .as_ref()
                            .map(|l| l.to_string())
                            .unwrap_or_else(|| "text".to_string()),
                        text.to_string(),
                    ),
                    _ => BlockContent::text(text.to_string()),
                };
                backend
                    .update_block(block_id, content)
                    .await
                    .map_err(|e| anyhow::anyhow!("update_block failed: {}", e))?;
            }
        } else if field_name == "marks" {
            // SQL→Loro inbound for marks: parse the JSON projection back to
            // Vec<MarkSpan> and apply via Peritext. Keeps the existing text;
            // a content+marks change arrives as two FieldsChanged tuples and
            // the content one will have been processed already.
            //
            // null → clear marks (wholesale replace with empty set).
            let new_marks: Vec<holon_api::MarkSpan> = match new_value {
                serde_json::Value::Null => Vec::new(),
                serde_json::Value::String(s) => holon_api::marks_from_json(s)
                    .map_err(|e| anyhow::anyhow!("marks FieldsChanged JSON parse error: {}", e))?,
                other => {
                    return Err(anyhow::anyhow!(
                        "marks FieldsChanged unexpected value shape: {:?}",
                        other
                    ));
                }
            };
            let existing = backend
                .get_block(block_id)
                .await
                .map_err(|e| anyhow::anyhow!("get_block({}) failed: {}", block_id, e))?;
            backend
                .update_block_marked(block_id, &existing.content, &new_marks)
                .await
                .map_err(|e| anyhow::anyhow!("update_block_marked failed: {}", e))?;
        } else if field_name == "parent_id" {
            // parent_id is a structural change — must go through the LoroTree
            // CRDT (`tree.mov` via `update_parent_id`), NOT the property map.
            // Without this, concurrent peer indents/moves can't merge as
            // tree CRDT moves; they merge as JSON property values, which
            // doesn't enforce tree consistency (no cycle detection).
            let parent_id_str = new_value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("parent_id FieldsChanged value is not a string"))?;
            backend
                .update_parent_id(block_id, parent_id_str.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("update_parent_id failed: {}", e))?;
        } else if field_name == "depth" {
            // depth is a derived field — the LoroTree encodes hierarchy
            // structurally, and depth is recomputed on outbound snapshot.
            // Skip writing it back into Loro to avoid drift.
        } else if field_name != "content_type" && field_name != "source_language" {
            let val = match new_value {
                serde_json::Value::String(s) => Value::String(s.clone()),
                serde_json::Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        Value::Integer(i)
                    } else {
                        Value::Float(n.as_f64().unwrap_or(0.0))
                    }
                }
                serde_json::Value::Bool(b) => Value::Boolean(*b),
                serde_json::Value::Null => Value::Null,
                _ => Value::String(new_value.to_string()),
            };
            props.insert(field_name.to_string(), val);
        }
    }

    if !props.is_empty() {
        backend
            .update_block_properties(block_id, &props)
            .await
            .map_err(|e| anyhow::anyhow!("update_properties failed: {}", e))?;
    }

    debug!("[LoroSyncController] FieldsChanged for {}", block_id);
    Ok(())
}

async fn apply_create(backend: &LoroBackend, data: &serde_json::Value) -> Result<()> {
    let block_id = json_str(data, "id")?;

    if backend.resolve_to_tree_id(block_id).await.is_some() {
        debug!(
            "[LoroSyncController] Block {} exists, updating instead",
            block_id
        );
        return apply_update_with_backend(backend, data).await;
    }

    let parent_id_raw = data
        .get("parent_id")
        .and_then(|v| v.as_str())
        .unwrap_or("sentinel:no_parent");

    let parent_uri = if backend.resolve_to_tree_id(parent_id_raw).await.is_some() {
        EntityUri::from_raw(parent_id_raw)
    } else {
        let parent_entity = EntityUri::from_raw(parent_id_raw);
        if parent_entity.is_no_parent() || parent_entity.is_sentinel() {
            parent_entity
        } else {
            let placeholder_uri = backend
                .create_placeholder_root(parent_entity.id())
                .await
                .map_err(|e| anyhow::anyhow!("create document placeholder failed: {}", e))?;
            info!(
                "[LoroSyncController] Created document placeholder for {} (uri={})",
                parent_id_raw, placeholder_uri
            );
            EntityUri::from_raw(&placeholder_uri)
        }
    };

    let content = content_from_json(data);
    let block_id_uri = EntityUri::from_raw(block_id);
    let created = backend
        .create_block(parent_uri, content, Some(block_id_uri))
        .await
        .map_err(|e| anyhow::anyhow!("create_block failed: {}", e))?;

    let name = data.get("name").and_then(|v| v.as_str());
    if name.is_some() {
        backend
            .set_document_metadata(created.id.as_str(), name)
            .await?;
    }

    apply_properties_from_json(backend, created.id.as_str(), data).await?;

    debug!("[LoroSyncController] Created {}", created.id);
    Ok(())
}

async fn apply_update_with_backend(backend: &LoroBackend, data: &serde_json::Value) -> Result<()> {
    let block_id = json_str(data, "id")?;

    if backend.resolve_to_tree_id(block_id).await.is_none() {
        warn!(
            "[LoroSyncController] Block {} not in Loro during update — \
             update will be lost. Indicates a seed/subscribe ordering bug \
             unless this is a document block.",
            block_id
        );
        return Ok(());
    }

    let content = content_from_json(data);
    backend
        .update_block(block_id, content)
        .await
        .map_err(|e| anyhow::anyhow!("update_block failed: {}", e))?;

    // parent_id is structural — must go through `tree.mov`. Non-Loro
    // origins (e.g. OrgSync) emit `EventKind::Updated` with full-row data
    // that may carry a parent_id change; without this branch the change
    // is silently dropped on the Loro side. Compare against current Loro
    // parent to skip no-op tree.mov calls.
    if let Some(new_parent_str) = data.get("parent_id").and_then(|v| v.as_str()) {
        let new_parent = EntityUri::from_raw(new_parent_str);
        let current = backend
            .get_block(block_id)
            .await
            .map_err(|e| anyhow::anyhow!("get_block({}) failed: {}", block_id, e))?;
        if current.parent_id != new_parent {
            backend
                .update_parent_id(block_id, new_parent_str.to_string())
                .await
                .map_err(|e| anyhow::anyhow!("update_parent_id failed: {}", e))?;
        }
    }

    apply_properties_from_json(backend, block_id, data).await?;

    debug!("[LoroSyncController] Updated {}", block_id);
    Ok(())
}

async fn apply_delete(backend: &LoroBackend, block_id: &str) -> Result<()> {
    if backend.resolve_to_tree_id(block_id).await.is_none() {
        warn!(
            "[LoroSyncController] Block {} not in Loro during delete — \
             already gone or never seeded.",
            block_id
        );
        return Ok(());
    }

    backend
        .delete_block(block_id)
        .await
        .map_err(|e| anyhow::anyhow!("delete_block failed: {}", e))?;

    debug!("[LoroSyncController] Deleted {}", block_id);
    Ok(())
}

fn json_str<'a>(data: &'a serde_json::Value, key: &str) -> Result<&'a str> {
    data.get(key)
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("Block data missing '{}'", key))
}

fn content_from_json(data: &serde_json::Value) -> BlockContent {
    let content = data.get("content").and_then(|v| v.as_str()).unwrap_or("");
    let content_type = data
        .get("content_type")
        .and_then(|v| v.as_str())
        .unwrap_or("text");

    if content_type == "source" {
        let lang = data
            .get("source_language")
            .and_then(|v| v.as_str())
            .unwrap_or("text");
        BlockContent::source(lang, content.to_string())
    } else {
        BlockContent::text(content.to_string())
    }
}

async fn apply_properties_from_json(
    backend: &LoroBackend,
    tree_id_str: &str,
    data: &serde_json::Value,
) -> Result<()> {
    let props = match data.get("properties") {
        Some(serde_json::Value::Object(map)) => {
            let converted: HashMap<String, Value> = map
                .iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => Value::String(s.clone()),
                        serde_json::Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                Value::Integer(i)
                            } else {
                                Value::Float(n.as_f64().unwrap_or(0.0))
                            }
                        }
                        serde_json::Value::Bool(b) => Value::Boolean(*b),
                        serde_json::Value::Null => Value::Null,
                        _ => Value::String(v.to_string()),
                    };
                    (k.clone(), val)
                })
                .collect();
            Some(converted)
        }
        Some(serde_json::Value::String(s)) => {
            serde_json::from_str::<HashMap<String, Value>>(s).ok()
        }
        _ => None,
    };

    if let Some(props) = props {
        if !props.is_empty() {
            backend
                .update_block_properties(tree_id_str, &props)
                .await
                .map_err(|e| anyhow::anyhow!("update_properties failed: {}", e))?;
        }
    }
    Ok(())
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
    use holon_api::{InlineMark, MarkSpan};

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

    #[test]
    fn diff_snapshots_emits_expected_marks_guard_on_change() {
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

        let expected = params
            .get("_expected_marks")
            .expect("_expected_marks present when marks changed");
        let s = expected.as_string().expect("expected_marks is String");
        let parsed: Vec<MarkSpan> = holon_api::marks_from_json(s).expect("parse pre-image");
        assert_eq!(parsed, old_marks);

        let new_val = params.get("marks").expect("new marks present");
        let s = new_val.as_string().expect("marks is String");
        let parsed: Vec<MarkSpan> = holon_api::marks_from_json(s).expect("parse new");
        assert_eq!(parsed, new_marks);
    }

    #[test]
    fn diff_snapshots_uses_empty_sentinel_when_pre_image_was_none() {
        let mut before = HashMap::new();
        let mut after = HashMap::new();
        let id = "block:b1".to_string();
        before.insert(id.clone(), block_with_marks("hi", None));
        after.insert(
            id.clone(),
            block_with_marks("hi", Some(vec![MarkSpan::new(0, 2, InlineMark::Bold)])),
        );

        let ops = diff_snapshots_to_ops(&before, &after);
        let (_, params) = ops
            .iter()
            .find(|(op, _)| op == "update")
            .expect("update op");

        let expected = params
            .get("_expected_marks")
            .expect("_expected_marks present");
        // Empty string sentinel signals "pre-image was None → SQL row had marks IS NULL".
        assert_eq!(*expected, Value::String(String::new()));
    }

    // -- SQL→Loro inbound apply tests ----------------------------------------

    use crate::api::repository::{CoreOperations, Lifecycle};
    use holon_api::BlockContent;
    use serde_json::json;

    async fn create_inbound_test_backend() -> LoroBackend {
        LoroBackend::create_new("inbound-test-doc".to_string())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn inbound_marks_applies_to_loro() {
        let backend = create_inbound_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text {
                    raw: "hello world".into(),
                },
                None,
            )
            .await
            .unwrap();

        let new_marks = vec![
            MarkSpan::new(0, 5, InlineMark::Bold),
            MarkSpan::new(6, 11, InlineMark::Italic),
        ];
        let marks_json = holon_api::marks_to_json(&new_marks);

        // Simulate a SQL→Loro FieldsChanged event for the marks column.
        let payload = json!([["marks", null, marks_json]]);
        apply_fields_changed(&backend, block.id.as_str(), &payload)
            .await
            .expect("apply_fields_changed marks");

        let fetched = backend.get_block(block.id.as_str()).await.unwrap();
        let mut got = fetched.marks.expect("marks projected after inbound");
        got.sort_by_key(|m| (m.start, m.end));
        assert_eq!(got, new_marks);
    }

    #[tokio::test]
    async fn inbound_marks_null_clears_marks() {
        let backend = create_inbound_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "abc".into() },
                None,
            )
            .await
            .unwrap();

        // First apply a Bold mark.
        backend
            .update_block_marked(
                block.id.as_str(),
                "abc",
                &[MarkSpan::new(0, 3, InlineMark::Bold)],
            )
            .await
            .expect("seed bold");
        let with_bold = backend.get_block(block.id.as_str()).await.unwrap();
        assert!(with_bold.marks.is_some());

        // Now simulate SQL→Loro inbound with null = clear marks.
        let payload = json!([["marks", "<old json>", null]]);
        apply_fields_changed(&backend, block.id.as_str(), &payload)
            .await
            .expect("apply_fields_changed marks=null");

        let cleared = backend.get_block(block.id.as_str()).await.unwrap();
        assert!(cleared.marks.is_none(), "expected None after inbound null");
    }

    #[tokio::test]
    async fn inbound_marks_invalid_json_surfaces_error() {
        let backend = create_inbound_test_backend().await;
        let block = backend
            .create_block(
                EntityUri::no_parent(),
                BlockContent::Text { raw: "hi".into() },
                None,
            )
            .await
            .unwrap();

        let payload = json!([["marks", null, "{not valid json"]]);
        let err = apply_fields_changed(&backend, block.id.as_str(), &payload)
            .await
            .expect_err("invalid JSON must be a hard error per fail-loud policy");
        let msg = format!("{err}");
        assert!(
            msg.contains("marks FieldsChanged JSON parse error"),
            "expected loud parse error, got: {msg}"
        );
    }
}

//! `StubSut` — an in-process `LoroSyncController` running against in-memory
//! stubs for `OperationProvider` and `EventBus`.
//!
//! The stub implementations live inside this module so the Layer 3 PBT
//! (`tests/loro_sync_controller_pbt.rs`) can wire up a complete controller
//! without pulling in Turso, the command bus, or any DI machinery.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use holon::core::datasource::{
    OperationDescriptor, OperationProvider, OperationResult, Result as DatasourceResult,
};
use holon::storage::types::Result as StorageResult;
use holon::storage::types::StorageEntity;
use holon::sync::event_bus::{
    CommandId, Event, EventBus, EventFilter, EventId, EventOrigin, EventStream,
};
use holon::sync::multi_peer::{GroupState, GroupTransition, sync_docs_direct};
use holon::sync::{LoroDocumentStore, LoroSyncController, LoroSyncControllerHandle};
use holon_api::EntityName;
use loro::Frontiers;
use tempfile::TempDir;
use tokio::sync::{Mutex, RwLock};

use super::{BlockSnapshot, LoroSyncSut};

/// In-process SUT: a `LoroSyncController` on a fresh `LoroDocumentStore`
/// backed by a `TempDir`, wired to a `StubOperationProvider` and a
/// `StubEventBus`. The downstream block store lives entirely in memory.
pub struct StubSut {
    /// Kept alive so the temp directory outlives the controller. Reading
    /// this field is intentional — it holds a `Drop` guard.
    _tempdir: Arc<TempDir>,
    storage_dir: PathBuf,
    doc_store: Arc<RwLock<LoroDocumentStore>>,
    controller_handle: Option<LoroSyncControllerHandle>,
    stub_ops: Arc<StubOperationProvider>,
    event_bus: Arc<StubEventBus>,
}

impl std::fmt::Debug for StubSut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StubSut")
            .field("storage_dir", &self.storage_dir)
            .field("has_controller", &self.controller_handle.is_some())
            .finish()
    }
}

impl StubSut {
    /// Construct a fresh `StubSut` with an initialized (but not yet started)
    /// primary Loro doc and a running `LoroSyncController` subscribed to it.
    pub async fn new() -> Result<Self> {
        let tempdir = Arc::new(tempfile::tempdir()?);
        let storage_dir = tempdir.path().to_path_buf();
        let doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(storage_dir.clone())));

        // Force the global doc to be created (with schema initialized) so
        // the controller's initial subscribe_root has something to hook
        // into.
        {
            let store = doc_store.read().await;
            let _ = store.get_global_doc().await?;
        }

        let stub_ops = Arc::new(StubOperationProvider::new());
        let event_bus = Arc::new(StubEventBus::new());

        let mut sut = Self {
            _tempdir: tempdir,
            storage_dir,
            doc_store,
            controller_handle: None,
            stub_ops,
            event_bus,
        };
        sut.start_controller().await?;
        Ok(sut)
    }

    /// (Re)start the `LoroSyncController`. Used by `Restart` and `OfflineMerge`.
    async fn start_controller(&mut self) -> Result<()> {
        let command_bus: Arc<dyn OperationProvider> = self.stub_ops.clone();
        let event_bus_arc: Arc<dyn EventBus> = self.event_bus.clone();
        let controller = LoroSyncController::new(
            self.doc_store.clone(),
            command_bus,
            event_bus_arc,
            self.storage_dir.clone(),
        );
        let handle = controller.start().await?;
        self.controller_handle = Some(handle);
        Ok(())
    }

    async fn stop_controller(&mut self) {
        // Dropping the handle dispatches its owned Subscription, which
        // cancels the `subscribe_root` callback. The background task is
        // detached but becomes idle once no more events or wakes arrive.
        self.controller_handle = None;
    }

    /// Mirror the reference state's peer 0 into the SUT's primary doc via
    /// a direct Loro sync. This is the "the production app just observed
    /// whatever peer 0 currently holds" operation; it's the standard way
    /// to propagate reference-state mutations into the SUT.
    async fn sync_primary_from_ref(&self, state: &GroupState<()>) -> Result<()> {
        if state.peers.is_empty() {
            return Ok(());
        }
        let ref_doc = &state.peers[0].doc;
        let store = self.doc_store.read().await;
        let collab = store
            .get_global_doc()
            .await
            .map_err(|e| anyhow::anyhow!("get_global_doc: {}", e))?;
        let doc_arc = collab.doc();
        let doc = doc_arc.write().await;
        sync_docs_direct(&doc, ref_doc);
        Ok(())
    }
}

#[async_trait]
impl LoroSyncSut for StubSut {
    async fn apply(&mut self, state: &GroupState<()>, transition: &GroupTransition) -> Result<()> {
        match transition {
            // Transitions that mutate peer 0 directly, or that merge into
            // peer 0, are the only ones the SUT needs to observe. For all
            // others, peer 0's doc is unchanged and `sync_primary_from_ref`
            // is a safe no-op.
            _ => {
                self.sync_primary_from_ref(state).await?;
            }
        }

        match transition {
            GroupTransition::Restart => {
                // Shut down the controller, persist the current primary
                // doc, then re-create the controller. The startup
                // reconcile runs against the persisted watermark.
                self.stop_controller().await;
                {
                    let store = self.doc_store.read().await;
                    store
                        .save_all()
                        .await
                        .map_err(|e| anyhow::anyhow!("save_all on Restart: {}", e))?;
                }
                // Re-create the store itself so the .loro file is reloaded
                // fresh from disk.
                self.doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
                    self.storage_dir.clone(),
                )));
                {
                    let store = self.doc_store.read().await;
                    let _ = store.get_global_doc().await?;
                }
                self.start_controller().await?;
            }

            GroupTransition::OfflineMerge { peer_idx } => {
                // Background-service scenario: shut down the controller,
                // merge the remote peer into the primary's on-disk doc
                // while the controller is dead, then restart.
                self.stop_controller().await;
                {
                    let store = self.doc_store.read().await;
                    let collab = store
                        .get_global_doc()
                        .await
                        .map_err(|e| anyhow::anyhow!("get_global_doc: {}", e))?;
                    let doc_arc = collab.doc();
                    let doc = doc_arc.write().await;
                    sync_docs_direct(&doc, &state.peers[*peer_idx].doc);
                    // The controller is down, so no subscribe_root fires.
                }
                {
                    let store = self.doc_store.read().await;
                    store
                        .save_all()
                        .await
                        .map_err(|e| anyhow::anyhow!("save_all on OfflineMerge: {}", e))?;
                }
                self.doc_store = Arc::new(RwLock::new(LoroDocumentStore::new(
                    self.storage_dir.clone(),
                )));
                {
                    let store = self.doc_store.read().await;
                    let _ = store.get_global_doc().await?;
                }
                self.start_controller().await?;
            }

            _ => {}
        }

        Ok(())
    }

    async fn wait_for_quiescence(&mut self) {
        let Some(handle) = self.controller_handle.as_ref() else {
            return;
        };
        // Poll: `last_synced == oplog_frontiers` means the controller has
        // caught up with every Loro mutation observed so far.
        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(5);
        loop {
            let current = self.primary_oplog_frontiers().await;
            let last = handle.last_synced_frontiers();
            if last == current {
                return;
            }
            if tokio::time::Instant::now() >= deadline {
                panic!(
                    "[StubSut::wait_for_quiescence] timeout: last={:?} current={:?} errors={}",
                    last,
                    current,
                    handle.error_count()
                );
            }
            tokio::time::sleep(tokio::time::Duration::from_millis(5)).await;
        }
    }

    async fn downstream_snapshot(&self) -> BTreeMap<String, BlockSnapshot> {
        self.stub_ops.snapshot().await
    }

    async fn last_synced_frontiers(&self) -> Frontiers {
        self.controller_handle
            .as_ref()
            .map(|h| h.last_synced_frontiers())
            .unwrap_or_default()
    }

    async fn primary_oplog_frontiers(&self) -> Frontiers {
        let store = self.doc_store.read().await;
        let Ok(collab) = store.get_global_doc().await else {
            return Frontiers::default();
        };
        let doc_arc = collab.doc();
        let doc = doc_arc.read().await;
        doc.oplog_frontiers()
    }

    async fn primary_loro_snapshot(&self) -> BTreeMap<String, BlockSnapshot> {
        let store = self.doc_store.read().await;
        let Ok(collab) = store.get_global_doc().await else {
            return BTreeMap::new();
        };
        let doc_arc = collab.doc();
        let doc = doc_arc.read().await;
        let blocks = holon::api::snapshot_blocks_from_doc(&doc);
        blocks
            .into_iter()
            .map(|(id, block)| {
                (
                    id.clone(),
                    BlockSnapshot {
                        id,
                        parent_id: block.parent_id.to_string(),
                        content: block.content,
                    },
                )
            })
            .collect()
    }

    fn error_count(&self) -> usize {
        self.controller_handle
            .as_ref()
            .map(|h| h.error_count())
            .unwrap_or(0)
    }
}

// -- Stub OperationProvider -----------------------------------------------

/// In-memory block store masquerading as an `OperationProvider`.
///
/// Only `execute_batch_with_origin` is implemented — that's the only path
/// `LoroSyncController` calls. Other methods panic loudly so we catch
/// unexpected usage.
pub struct StubOperationProvider {
    /// Primary block store: stable_id → BlockSnapshot.
    blocks: Mutex<BTreeMap<String, BlockSnapshot>>,
    /// Number of batches received (for sanity asserts in tests).
    pub batches_received: Mutex<usize>,
}

impl StubOperationProvider {
    pub fn new() -> Self {
        Self {
            blocks: Mutex::new(BTreeMap::new()),
            batches_received: Mutex::new(0),
        }
    }

    pub async fn snapshot(&self) -> BTreeMap<String, BlockSnapshot> {
        self.blocks.lock().await.clone()
    }
}

#[async_trait]
impl OperationProvider for StubOperationProvider {
    fn operations(&self) -> Vec<OperationDescriptor> {
        Vec::new()
    }

    async fn execute_operation(
        &self,
        _entity_name: &EntityName,
        _op_name: &str,
        _params: StorageEntity,
    ) -> DatasourceResult<OperationResult> {
        panic!(
            "StubOperationProvider::execute_operation is not implemented; use execute_batch_with_origin"
        );
    }

    async fn execute_batch_with_origin(
        &self,
        entity_name: &EntityName,
        operations: Vec<(String, StorageEntity)>,
        _origin: EventOrigin,
    ) -> DatasourceResult<Vec<OperationResult>> {
        assert_eq!(
            entity_name, "block",
            "StubOperationProvider only knows the 'block' entity"
        );
        let mut blocks = self.blocks.lock().await;
        for (op_name, params) in &operations {
            match op_name.as_str() {
                "create" | "update" => {
                    let id = params
                        .get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .expect("create/update missing 'id'");
                    let parent_id = params
                        .get("parent_id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    let content = params
                        .get("content")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .unwrap_or_default();
                    blocks.insert(
                        id.clone(),
                        BlockSnapshot {
                            id,
                            parent_id,
                            content,
                        },
                    );
                }
                "delete" => {
                    let id = params
                        .get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .expect("delete missing 'id'");
                    blocks.remove(&id);
                }
                other => panic!("StubOperationProvider: unknown op '{}'", other),
            }
        }
        *self.batches_received.lock().await += 1;
        Ok(vec![
            OperationResult::irreversible(Vec::new());
            operations.len()
        ])
    }
}

// -- Stub EventBus --------------------------------------------------------

/// A minimal no-op `EventBus`. The bridge PBT does not inject inbound
/// events in v1 — it only exercises the outbound (Loro → command bus)
/// direction via Loro doc mutations. This stub satisfies the trait so
/// `LoroSyncController::start` can call `subscribe` without plumbing a
/// real event bus.
pub struct StubEventBus {
    /// The sender end of the subscription channel is held here so the
    /// stub can optionally inject events into the controller's inbound
    /// branch. v1 doesn't exercise that — the channel exists but is never
    /// sent into from outside.
    tx: Mutex<Option<tokio::sync::mpsc::Sender<Event>>>,
}

impl StubEventBus {
    pub fn new() -> Self {
        Self {
            tx: Mutex::new(None),
        }
    }
}

#[async_trait]
impl EventBus for StubEventBus {
    async fn publish(
        &self,
        _event: Event,
        _command_id: Option<CommandId>,
    ) -> StorageResult<EventId> {
        // The controller only ever publishes via `execute_batch_with_origin`,
        // which goes through the stub `OperationProvider`. A real EventBus
        // publish is never called in the stub path.
        Ok("stub".to_string())
    }

    async fn subscribe(&self, _filter: EventFilter) -> StorageResult<EventStream> {
        let (tx, rx) = tokio::sync::mpsc::channel(16);
        *self.tx.lock().await = Some(tx);
        Ok(tokio_stream::wrappers::ReceiverStream::new(rx))
    }

    async fn mark_processed(&self, _event_id: &EventId, _consumer: &str) -> StorageResult<()> {
        Ok(())
    }

    async fn update_status(
        &self,
        _event_id: &EventId,
        _status: holon::sync::event_bus::EventStatus,
        _rejection_reason: Option<String>,
    ) -> StorageResult<()> {
        Ok(())
    }

    async fn link_speculative(
        &self,
        _confirmed_event_id: &EventId,
        _speculative_event_id: &EventId,
    ) -> StorageResult<()> {
        Ok(())
    }
}

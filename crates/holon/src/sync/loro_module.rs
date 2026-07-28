//! Standalone Loro DI module
//!
//! Registers Loro CRDT services independently of OrgMode. When enabled,
//! Loro provides:
//! - `LoroDocumentStore` for managing CRDT documents
//! - `LoroBlocksDataSource` for populating `QueryableCache`
//! - `LoroBlockOperations` for direct Loro CRDT access (not registered as
//!   `OperationProvider`)
//! - `LoroSyncController` — the outbound Loro → SQL projector. Subscribes to
//!   `doc.subscribe_root` and projects each Loro change into `block_raw`. Loro
//!   is the authority (seeded from the bundled Org assets via intents); there
//!   is no SQL→Loro direction.

use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::Provider;
use fluxdi::Shared;
use holon_core::OriginTaggedWrites;
use holon_turso::schema_modules::BlockSchemaModule;
use tokio::sync::RwLock;
use tracing::error;
use tracing::info;

use crate::core::SqlOperationProvider;
use crate::storage::BLOCK_WRITE_TABLE;
use crate::storage::schema_module::SchemaModule;
use crate::sync::LoroBlockOperations;
use crate::sync::LoroBlocksDataSource;
use crate::sync::LoroDocumentStore;
use crate::sync::LoroSyncController;
use crate::sync::LoroSyncControllerHandle;

/// Configuration for standalone Loro CRDT support
#[derive(Clone, Debug)]
pub struct LoroConfig {
    /// Root directory for Loro document storage
    pub storage_dir: PathBuf,
    /// Peer id this session's global doc is minted under. `None` = the
    /// env/random fallback. Injected by `SessionConfig::loro_peer_id` so two
    /// sessions in one process (the two-instance sharing PBT) never collide.
    pub peer_id: Option<u64>,
}

impl LoroConfig {
    pub fn new(storage_dir: PathBuf) -> Self {
        let storage_dir = std::fs::canonicalize(&storage_dir).unwrap_or(storage_dir);
        Self {
            storage_dir,
            peer_id: None,
        }
    }

    pub fn with_peer_id(mut self, peer_id: Option<u64>) -> Self {
        self.peer_id = peer_id;
        self
    }
}

/// ServiceModule for standalone Loro CRDT support
///
/// Registers Loro-specific services in the DI container without requiring
/// OrgMode. When both OrgMode and Loro are enabled, OrgMode's DI should detect
/// that LoroBlockOperations is already registered and use it instead of
/// creating its own.
pub struct LoroModule;

impl Module for LoroModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        info!("[LoroModule] register_services called");

        // Register LoroDocumentStore
        injector.provide::<LoroDocumentStore>(Provider::root(|resolver| {
            let config = resolver.resolve::<LoroConfig>();
            Shared::new(
                LoroDocumentStore::new(config.storage_dir.clone()).with_peer_id(config.peer_id),
            )
        }));

        // Register LoroBlocksDataSource
        injector.provide::<LoroBlocksDataSource>(Provider::root(|resolver| {
            let doc_store = resolver.resolve::<LoroDocumentStore>();
            Shared::new(LoroBlocksDataSource::new(Arc::new(RwLock::new(
                (*doc_store).clone(),
            ))))
        }));

        // Register LoroBlockOperations. When the subtree-share machinery is
        // compiled in, thread the `SharedTreeSyncManager` (the `SharedTreeStore`)
        // into the block-ops write path so writes to blocks that were pruned
        // into a shared subtree doc route there instead of silently no-op-ing on
        // the global doc (B3: mount-aware write routing). The manager provider is
        // registered by `register_subtree_share` in this same `configure`; both
        // are lazy factories, so registration order within `configure` is moot.
        injector.provide::<LoroBlockOperations>(Provider::root(|resolver| {
            let doc_store = resolver.resolve::<LoroDocumentStore>();
            let ops = LoroBlockOperations::new(Arc::new(RwLock::new((*doc_store).clone())));
            #[cfg(all(
                feature = "iroh-sync",
                not(all(target_arch = "wasm32", target_os = "unknown"))
            ))]
            let ops = {
                use holon_loro::shared_tree::SharedTreeStore;

                use crate::sync::iroh_sync_adapter::SharedTreeSyncManager;
                let manager = resolver.resolve::<Arc<SharedTreeSyncManager>>();
                ops.with_shared_trees((*manager).clone() as Arc<dyn SharedTreeStore>)
            };
            Shared::new(ops)
        }));

        // Register a Loro-aware `BlockCellRegistry`. `SqlBlockOperations`
        // consumes this so chord-time `split_block` / `join_block` reads
        // the live `content_raw` `LoroText` through `Cell<String>` rather
        // than the SQL `block.content` projection — closing the
        // typed-text-discarded race documented in
        // `devlog/2026-05-08-154449-split-block-discards-pending-edits.md`.
        // Only registered when LoroModule is loaded; SqlOnly mode wires
        // `BlockCellRegistry::sql_only()` in `event_infra_module.rs`.
        injector.provide::<crate::sync::block_cell_registry::BlockCellRegistry>(
            Provider::root_async(|resolver| async move {
                let doc_store = resolver.resolve::<LoroDocumentStore>();
                let collab = doc_store
                    .get_global_doc()
                    .await
                    .expect("LoroDocumentStore::get_global_doc failed for BlockCellRegistry");
                Shared::new(crate::sync::block_cell_registry::BlockCellRegistry::with_loro(collab))
            }),
        );

        // Loro is the source of truth for block CRUD. `LoroBlockOperations`
        // (registered above as a concrete type) is wired into the UI command
        // path by `OrgModeModule` (`holon-orgmode/src/di.rs`): when Loro is
        // enabled it becomes the block CRUD `OperationProvider`, so set_field /
        // create / update / delete land in the Loro doc. `LoroSyncController`
        // then projects each Loro change to the SQL `block_raw` table (a pure
        // projection — there is no SQL→Loro mirror). `CacheBlockReader` reads
        // the resulting `QueryableCache`, which stays consistent because it is
        // fed from that same projection. In SqlOnly mode (no LoroModule) the
        // generic `SqlOperationProvider` owns block CRUD and SQL is authority.

        // Wire up `LoroSyncController` — the bidirectional bridge between
        // the Loro doc and the abstract command/event bus. Registered as a
        // root factory to defer execution until DI resolution. The handle
        // owns the Loro subscription and the background task; keeping this
        // value in DI keeps both alive.
        // The downstream Loro→SQL projection (consolidator → SQL sink convergent
        // feed) as a SHARED, standalone DI service. Built independently of the
        // controller's run-loop task so org's initial scan can drive it (via
        // `DownstreamProjection::flush`) WITHOUT resolving the controller handle
        // — that handle's factory runs `seed` + `controller.start()`, which is
        // gated post-scan. The controller resolves the SAME instance, so the
        // run loop and org's flush advance one `last_synced` watermark and
        // serialize on the projection's lock. `last_synced` loads from the
        // sidecar (last session's frontier); Loro's persisted snapshot is the
        // startup source of truth, so the diff is bounded to this session's
        // changes.
        injector.provide::<crate::sync::loro_sync_controller::LoroProjection>(
            Provider::root_async(|resolver| async move {
                let config = resolver.resolve::<LoroConfig>();
                let doc_store = resolver.resolve::<LoroDocumentStore>();
                let db_handle_provider = resolver.resolve::<dyn crate::di::DbHandleProvider>();
                let db_handle = db_handle_provider.handle();
                let sql_ops = Arc::new(SqlOperationProvider::with_edge_fields(
                    db_handle.clone(),
                    BLOCK_WRITE_TABLE.to_string(),
                    "block".to_string(),
                    "block".to_string(),
                    BlockSchemaModule.edge_fields(),
                ));
                let command_bus: Arc<dyn OriginTaggedWrites> = sql_ops;
                let sink_reader: Arc<dyn crate::sync::SinkReader> =
                    Arc::new(crate::storage::TursoSinkReader::new(db_handle));
                let doc_store_arc = Arc::new(RwLock::new((*doc_store).clone()));
                Shared::new(
                    crate::sync::loro_sync_controller::LoroProjection::from_storage(
                        doc_store_arc,
                        command_bus,
                        sink_reader,
                        &config.storage_dir,
                    ),
                )
            }),
        );

        injector.provide::<dyn holon_core::DownstreamProjection>(Provider::root_async(
            |resolver| async move {
                let projection = resolver
                    .resolve_async::<crate::sync::loro_sync_controller::LoroProjection>()
                    .await;
                projection as Arc<dyn holon_core::DownstreamProjection>
            },
        ));

        tracing::info!(
            "[LoroModule] STAGE 1: registering LoroSyncControllerHandle provider (pre-provide \
             call)"
        );
        injector.provide::<LoroSyncControllerHandle>(Provider::root_async(|resolver| async move {
            tracing::info!(
                "[LoroModule] STAGE 2: LoroSyncControllerHandle factory body started (inside \
                 async closure)"
            );
            info!("[LoroModule] LoroSyncControllerHandle factory: entering");
            let doc_store = resolver.resolve::<LoroDocumentStore>();
            tracing::info!("[LoroModule] STAGE 3: upstream deps resolved");
            info!("[LoroModule] LoroSyncControllerHandle factory: upstream deps resolved");

            // The Loro controller writes to the persistent block store
            // through an `OperationProvider`. We construct a dedicated
            // `SqlOperationProvider` instance for it — equivalent to the
            // one OrgMode uses, but independent so the two directions
            // can run in parallel without coupling.
            // The downstream Loro→SQL projection is a shared standalone service
            // (registered above). Resolve the SAME instance org's scan flushes,
            // so the controller's run loop and org's flush advance one
            // `last_synced` watermark and serialize on the projection's lock.
            let projection = resolver
                .resolve_async::<crate::sync::loro_sync_controller::LoroProjection>()
                .await;
            tracing::info!("[LoroModule] STAGE 3e: shared projection resolved");

            let doc_store_arc = Arc::new(RwLock::new((*doc_store).clone()));
            tracing::info!("[LoroModule] STAGE 3f: doc_store_arc built");

            // Loro is NOT seeded from the persistent (Turso) store. The bundled
            // Org assets are the seed source: they reach Loro directly via
            // intents (`BlockOrdering::create_in_tree`) — `Journals.org` through
            // the file watcher's `on_file_changed`, the `index.org` layout +
            // `__default__` page through `FrontendSession::seed_default_layout`.
            // SQL (`block_raw`) is a pure projection of Loro. There is no
            // SQL→Loro direction (the Turso-seed + runtime mirror are removed).

            // Advance the shared projection watermark to the current frontiers so
            // the controller's first reconcile starts from the loaded doc state.
            {
                let frontiers = {
                    let store = doc_store_arc.read().await;
                    let collab = store
                        .get_global_doc()
                        .await
                        .expect("[LoroModule] get_global_doc for watermark advance");
                    collab.doc().oplog_frontiers()
                };
                *projection.last_synced().lock().unwrap() = frontiers;
            }

            // Arm the projection's DELETE pass now that the seed has mirrored
            // every persistent-store block (including the raw-inserted seed
            // layout) into Loro. Until this point the org initial scan may have
            // flushed the projection with Loro not yet holding the seed layout;
            // an unarmed projection withholds deletes so those SQL-only seed
            // rows (journals / root-layout / sidebar) are not spuriously
            // deleted before the seed reconciles them. See
            // `LoroProjection::arm`.
            projection.arm();

            // Rehydrate any previously-persisted shared subtrees —
            // walk mount nodes in the global doc, load each
            // `shares/<id>.loro` snapshot, re-register with the
            // manager + advertiser, attach save workers. Must run
            // AFTER the global doc is fully loaded but BEFORE the
            // sync controller starts, so the controller's first pass
            // sees a consistent share registry.
            #[cfg(all(
                feature = "iroh-sync",
                not(all(target_arch = "wasm32", target_os = "unknown"))
            ))]
            {
                use crate::sync::loro_share_backend::LoroShareBackend;
                use crate::sync::loro_share_backend::rehydrate_shared_trees;
                let backend = resolver.resolve::<Arc<LoroShareBackend>>();
                let store = doc_store_arc.read().await;
                let collab = store
                    .get_global_doc()
                    .await
                    .expect("[LoroModule] get_global_doc for share rehydration");
                let doc_arc = collab.doc();
                let doc = &*doc_arc;
                match rehydrate_shared_trees(&backend, doc).await {
                    Ok(n) if n > 0 => info!("[LoroModule] rehydrated {n} shared subtree(s)"),
                    Ok(_) => {}
                    Err(e) => {
                        error!("[LoroModule] rehydrate_shared_trees failed: {e:#}")
                    }
                }
            }

            let controller = LoroSyncController::new(doc_store_arc, projection);

            // Phase 4: resolve the shared convergent block feed (built once in
            // `EventInfraModule` as `BlockFeed`, available in both modes) and
            // hand it to the controller, which holds it to keep the CDC actor
            // alive. The same feed drives `block_link` via the link indexer —
            // one feed, many sinks (Phase 4b).
            let block_live = resolver
                .resolve_async::<holon_api::live_data::BlockFeed>()
                .await
                .0
                .clone();

            match controller.start(block_live).await {
                Ok(handle) => Shared::new(handle),
                Err(e) => {
                    error!("[LoroModule] Failed to start LoroSyncController: {}", e);
                    // Startup failure: return a handle to a controller
                    // that was never started. Tests will catch this via
                    // the error_count accessor on the handle (which
                    // stays at 0 for a dead controller).
                    panic!("LoroSyncController startup failed: {}", e);
                }
            }
        }));

        #[cfg(all(
            feature = "iroh-sync",
            not(all(target_arch = "wasm32", target_os = "unknown"))
        ))]
        register_subtree_share(injector);

        info!("[LoroModule] register_services complete");
        Ok(())
    }
}

#[cfg(all(
    feature = "iroh-sync",
    not(all(target_arch = "wasm32", target_os = "unknown"))
))]
fn register_subtree_share(injector: &Injector) {
    use holon_core::OperationProvider;
    use iroh::SecretKey;

    use crate::sync::iroh_advertiser::IrohAdvertiser;
    use crate::sync::iroh_sync_adapter::SharedTreeSyncManager;
    use crate::sync::loro_share_backend::LoroShareBackend;

    injector.provide::<Arc<SharedTreeSyncManager>>(Provider::root(|_| {
        Shared::new(Arc::new(SharedTreeSyncManager::new()))
    }));
    // Persistent device key loaded from `<storage_dir>/device.key`, or
    // generated + saved atomically on first launch. Identity must not
    // rotate across restarts — it's an input to `stable_peer_id` for
    // every shared Loro doc, AND it binds every iroh endpoint so
    // known-peer dedup on the remote side works across restarts.
    injector.provide::<Arc<SecretKey>>(Provider::root(|resolver| {
        let config = resolver.resolve::<LoroConfig>();
        let key = crate::sync::device_key_store::load_or_create_device_key(&config.storage_dir)
            .expect("load_or_create_device_key");
        Shared::new(Arc::new(key))
    }));
    injector.provide::<Arc<IrohAdvertiser>>(Provider::root(|resolver| {
        let key = resolver.resolve::<Arc<SecretKey>>();
        Shared::new(Arc::new(IrohAdvertiser::new_with_key((**key).clone())))
    }));
    injector.provide::<Arc<crate::sync::degraded_signal_bus::DegradedSignalBus>>(Provider::root(
        |_| {
            Shared::new(Arc::new(
                crate::sync::degraded_signal_bus::DegradedSignalBus::new(),
            ))
        },
    ));
    injector.provide::<Arc<crate::sync::shared_snapshot_store::SharedSnapshotStore>>(
        Provider::root(|resolver| {
            let config = resolver.resolve::<LoroConfig>();
            let bus =
                resolver.resolve::<Arc<crate::sync::degraded_signal_bus::DegradedSignalBus>>();
            Shared::new(Arc::new(
                crate::sync::shared_snapshot_store::SharedSnapshotStore::new(
                    config.storage_dir.clone(),
                    (*bus).clone(),
                ),
            ))
        }),
    );

    injector.provide::<Arc<LoroShareBackend>>(Provider::root_async(|resolver| async move {
        let doc_store = resolver.resolve::<LoroDocumentStore>();
        let snapshot_store =
            resolver.resolve::<Arc<crate::sync::shared_snapshot_store::SharedSnapshotStore>>();
        let manager = resolver.resolve::<Arc<SharedTreeSyncManager>>();
        let advertiser = resolver.resolve::<Arc<IrohAdvertiser>>();
        let bus = resolver.resolve::<Arc<crate::sync::degraded_signal_bus::DegradedSignalBus>>();
        let key = resolver.resolve::<Arc<SecretKey>>();
        let store_arc = Arc::new(RwLock::new((*doc_store).clone()));

        // Wire up the `block` SQL provider so mount-node projection into
        // the SQL `block` table works. Mirrors the construction in
        // `LoroModule::configure` — separate instance, but points at the
        // same `DbHandle`. Writes go to `block_raw`; `block` is a matview
        // and Turso rejects DML against it.
        let db_handle_provider = resolver.resolve::<dyn crate::di::DbHandleProvider>();
        let sql_ops = Arc::new(SqlOperationProvider::new(
            db_handle_provider.handle(),
            BLOCK_WRITE_TABLE.to_string(),
            "block".to_string(),
            "block".to_string(),
        ));

        // The global Loro→SQL projection (same instance `LoroSyncController`
        // drives). `share_subtree` flushes it after pruning so the global
        // prune-delete is applied to SQL before the sharer re-projects the
        // shared subtree under the mount — otherwise the delete races and
        // re-removes the just-re-created rows.
        let downstream_projection = resolver
            .resolve_async::<dyn holon_core::DownstreamProjection>()
            .await;

        // `LoroShareBackend::new_with_sql` returns `Arc<Self>` because its
        // internal `self_weak` is populated via `Arc::new_cyclic` — the
        // Arc has to exist to carry the Weak. Callers store the Arc as-is.
        Shared::new(LoroShareBackend::new_with_sql(
            store_arc,
            (*snapshot_store).clone(),
            (*manager).clone(),
            (*advertiser).clone(),
            (*bus).clone(),
            (**key).clone(),
            Some(sql_ops),
            Some(downstream_projection),
        ))
    }));

    injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
        |resolver| async move {
            let backend = resolver.resolve_async::<Arc<LoroShareBackend>>().await;
            (*backend).clone() as Arc<dyn OperationProvider>
        },
    ));
}

//! Standalone Loro DI module
//!
//! Registers Loro CRDT services independently of OrgMode. When enabled,
//! Loro provides:
//! - `LoroDocumentStore` for managing CRDT documents
//! - `LoroBlocksDataSource` for populating `QueryableCache`
//! - `LoroBlockOperations` for direct Loro CRDT access (not registered as
//!   `OperationProvider` — the command bus writes through SQL)
//! - `LoroSyncController` — the bidirectional bridge between the Loro doc
//!   and the abstract command/event bus. Subscribes to EventBus for
//!   inbound events and to `doc.subscribe_root` for outbound changes.

use std::path::PathBuf;
use std::sync::Arc;

use fluxdi::{Injector, Module, Provider, Shared};
use tokio::sync::RwLock;
use tracing::{error, info};

use crate::api::{CoreOperations, LoroBackend};
use crate::core::SqlOperationProvider;
use crate::core::datasource::OperationProvider;
use crate::core::queryable_cache::QueryableCache;
use crate::storage::DbHandle;
use crate::storage::schema_module::SchemaModule;
use crate::storage::schema_modules::BlockSchemaModule;
use crate::sync::event_bus::EventBus;
use crate::sync::{
    LoroBlockOperations, LoroBlocksDataSource, LoroDocumentStore, LoroSyncController,
    LoroSyncControllerHandle, TursoEventBus,
};
use holon_api::block::{Block, BlockContent};
use holon_api::{ContentType, EntityUri, Value};

/// Configuration for standalone Loro CRDT support
#[derive(Clone, Debug)]
pub struct LoroConfig {
    /// Root directory for Loro document storage
    pub storage_dir: PathBuf,
}

impl LoroConfig {
    pub fn new(storage_dir: PathBuf) -> Self {
        let storage_dir = std::fs::canonicalize(&storage_dir).unwrap_or(storage_dir);
        Self { storage_dir }
    }
}

/// ServiceModule for standalone Loro CRDT support
///
/// Registers Loro-specific services in the DI container without requiring OrgMode.
/// When both OrgMode and Loro are enabled, OrgMode's DI should detect that
/// LoroBlockOperations is already registered and use it instead of creating its own.
pub struct LoroModule;

impl Module for LoroModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        info!("[LoroModule] register_services called");

        // Register LoroDocumentStore
        injector.provide::<LoroDocumentStore>(Provider::root(|resolver| {
            let config = resolver.resolve::<LoroConfig>();
            Shared::new(LoroDocumentStore::new(config.storage_dir.clone()))
        }));

        // Register LoroBlocksDataSource
        injector.provide::<LoroBlocksDataSource>(Provider::root(|resolver| {
            let doc_store = resolver.resolve::<LoroDocumentStore>();
            Shared::new(LoroBlocksDataSource::new(Arc::new(RwLock::new(
                (*doc_store).clone(),
            ))))
        }));

        // Register LoroBlockOperations
        injector.provide::<LoroBlockOperations>(Provider::root(|resolver| {
            let doc_store = resolver.resolve::<LoroDocumentStore>();
            let cache = resolver.resolve::<QueryableCache<Block>>();
            Shared::new(LoroBlockOperations::new(
                Arc::new(RwLock::new((*doc_store).clone())),
                cache,
            ))
        }));

        // NOTE: LoroBlockOperations is NOT registered as an OperationProvider.
        // All block CRUD operations go through SqlOperationProvider → Turso (source of truth).
        // Loro is populated via EventBus subscriptions (reverse sync), not through the command path.
        // This ensures read/write consistency: CacheBlockReader reads from QueryableCache (backed by SQL), SqlOperationProvider writes to SQL.

        // Wire up `LoroSyncController` — the bidirectional bridge between
        // the Loro doc and the abstract command/event bus. Registered as a
        // root factory to defer execution until DI resolution. The handle
        // owns the Loro subscription and the background task; keeping this
        // value in DI keeps both alive.
        tracing::info!(
            "[LoroModule] STAGE 1: registering LoroSyncControllerHandle provider (pre-provide call)"
        );
        injector.provide::<LoroSyncControllerHandle>(Provider::root_async(|resolver| async move {
             tracing::info!(
                "[LoroModule] STAGE 2: LoroSyncControllerHandle factory body started (inside async closure)"
            );
            info!("[LoroModule] LoroSyncControllerHandle factory: entering");
            let config = resolver.resolve::<LoroConfig>();
            let doc_store = resolver.resolve::<LoroDocumentStore>();
            let event_bus = resolver.resolve::<TursoEventBus>();
             tracing::info!("[LoroModule] STAGE 3: upstream deps resolved");
            info!("[LoroModule] LoroSyncControllerHandle factory: upstream deps resolved");
            let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();
             tracing::info!("[LoroModule] STAGE 3a: event_bus_arc built");

            // The Loro controller writes to the persistent block store
            // through an `OperationProvider`. We construct a dedicated
            // `SqlOperationProvider` instance for it — equivalent to the
            // one OrgMode uses, but independent so the two directions
            // can run in parallel without coupling.
             tracing::info!("[LoroModule] STAGE 3b: resolving DbHandleProvider");
            let db_handle_provider = resolver.resolve::<dyn crate::di::DbHandleProvider>();
             tracing::info!("[LoroModule] STAGE 3c: DbHandleProvider resolved");
            let db_handle = db_handle_provider.handle();
             tracing::info!("[LoroModule] STAGE 3d: db_handle obtained");
            let sql_ops = Arc::new(SqlOperationProvider::with_event_bus_and_edge_fields(
                db_handle.clone(),
                "block".to_string(),
                "block".to_string(),
                "block".to_string(),
                event_bus_arc.clone(),
                BlockSchemaModule.edge_fields(),
            ));
            let command_bus: Arc<dyn OperationProvider> = sql_ops as Arc<dyn OperationProvider>;
             tracing::info!("[LoroModule] STAGE 3e: sql_ops built");

            let doc_store_arc = Arc::new(RwLock::new((*doc_store).clone()));
             tracing::info!("[LoroModule] STAGE 3f: doc_store_arc built; about to call seed");

            // Seed Loro from the persistent block store BEFORE starting
            // the controller. Some blocks enter SQL via raw writes that
            // bypass the `OperationProvider` entirely (notably
            // `seed_default_layout`, which has a legitimate bootstrap
            // reason to do so). Without this step those blocks would
            // never reach Loro — the controller's inbound branch only
            // sees EventBus events, and these blocks produce none.
            //
            // Seeding here runs inside the DI factory (before the
            // controller starts), so the controller sees Loro already
            // in sync with the persistent store and its initial
            // watermark → current-frontiers reconcile is a no-op.
            info!("[LoroModule] LoroSyncControllerHandle factory: calling seed_loro_from_persistent_store");
            if let Err(e) = seed_loro_from_persistent_store(&doc_store_arc, &db_handle).await {
                error!("[LoroModule] seed_loro_from_persistent_store failed: {}", e);
            }
            info!("[LoroModule] LoroSyncControllerHandle factory: seed call returned");

            // Pre-seed the sidecar with the current frontiers so the
            // controller doesn't diff seeded blocks against an empty
            // watermark (which would re-publish them as redundant creates).
            #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
            {
                let store = doc_store_arc.read().await;
                let collab = store
                    .get_global_doc()
                    .await
                    .expect("[LoroModule] get_global_doc for sidecar pre-seed");
                let frontiers = collab.doc().oplog_frontiers();
                let sidecar_path = config
                    .storage_dir
                    .join(super::loro_sync_controller::SIDECAR_FILENAME);
                if let Some(parent) = sidecar_path.parent() {
                    std::fs::create_dir_all(parent)
                        .expect("[LoroModule] create sidecar parent dir");
                }
                std::fs::write(&sidecar_path, frontiers.encode())
                    .expect("[LoroModule] write sidecar pre-seed");
            }

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
                use crate::sync::loro_share_backend::{LoroShareBackend, rehydrate_shared_trees};
                let backend = resolver.resolve::<Arc<LoroShareBackend>>();
                let store = doc_store_arc.read().await;
                let collab = store
                    .get_global_doc()
                    .await
                    .expect("[LoroModule] get_global_doc for share rehydration");
                let doc_arc = collab.doc();
                let doc = &*doc_arc;
                match rehydrate_shared_trees(&backend, &doc).await {
                    Ok(n) if n > 0 => info!("[LoroModule] rehydrated {n} shared subtree(s)"),
                    Ok(_) => {}
                    Err(e) => {
                        error!("[LoroModule] rehydrate_shared_trees failed: {e:#}")
                    }
                }
            }

            let controller = LoroSyncController::new(
                doc_store_arc,
                command_bus,
                event_bus_arc,
                config.storage_dir.clone(),
            );

            match controller.start().await {
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
    use crate::core::datasource::OperationProvider;
    use crate::sync::iroh_advertiser::IrohAdvertiser;
    use crate::sync::iroh_sync_adapter::SharedTreeSyncManager;
    use crate::sync::loro_share_backend::LoroShareBackend;
    use iroh::SecretKey;

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
        // same `DbHandle` and `TursoEventBus`, so the events flow through
        // a single bus into `CacheEventSubscriber`. `TursoEventBus` is
        // registered via `Provider::root_async`, so the factory must also
        // be async to resolve it.
        let db_handle_provider = resolver.resolve::<dyn crate::di::DbHandleProvider>();
        let event_bus = resolver.resolve_async::<TursoEventBus>().await;
        let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();
        let sql_ops = Arc::new(SqlOperationProvider::with_event_bus(
            db_handle_provider.handle(),
            "block".to_string(),
            "block".to_string(),
            "block".to_string(),
            event_bus_arc,
        ));

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
        ))
    }));

    injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
        |resolver| async move {
            let backend = resolver.resolve_async::<Arc<LoroShareBackend>>().await;
            (*backend).clone() as Arc<dyn OperationProvider>
        },
    ));
}

/// One-shot seed that copies every block currently in the persistent block
/// store into Loro. Used at startup to ensure Loro mirrors the bootstrap
/// state written by paths that bypass the `OperationProvider` (e.g.
/// `seed_default_layout`).
///
/// Idempotent: `create_block` with an existing stable ID is skipped, so
/// repeated invocations (e.g. across restarts) are safe.
pub async fn seed_loro_from_persistent_store(
    doc_store: &Arc<RwLock<LoroDocumentStore>>,
    db_handle: &DbHandle,
) -> anyhow::Result<()> {
    tracing::info!("[LoroModule] SEED-STAGE 1: function entry");
    info!("[LoroModule] seed: querying block table");
    tracing::info!("[LoroModule] SEED-STAGE 2: about to query block table");
    let rows = db_handle
        .query(
            "SELECT id, parent_id, content, content_type, source_language, \
                    properties \
             FROM block ORDER BY created_at ASC",
            std::collections::HashMap::new(),
        )
        .await
        .map_err(|e| anyhow::anyhow!("query block table: {}", e))?;

    tracing::info!(
        "[LoroModule] SEED-STAGE 3: query returned {} rows",
        rows.len()
    );
    info!(
        "[LoroModule] seed: got {} rows from block table",
        rows.len()
    );
    if rows.is_empty() {
        return Ok(());
    }

    let store = doc_store.read().await;
    let collab = store
        .get_global_doc()
        .await
        .map_err(|e| anyhow::anyhow!("get_global_doc: {}", e))?;
    let backend = LoroBackend::from_document(collab);

    let mut applied = 0usize;
    // Two-pass seed so children whose parent appears later in the result
    // set still get placed correctly.
    let mut pending: Vec<&std::collections::HashMap<String, Value>> = rows.iter().collect();
    for _pass in 0..2 {
        let mut next: Vec<&std::collections::HashMap<String, Value>> = Vec::new();
        for row in pending.drain(..) {
            match apply_seed_row(&backend, row).await {
                Ok(true) => applied += 1,
                Ok(false) => {}
                Err(_) => next.push(row),
            }
        }
        if next.is_empty() {
            break;
        }
        pending = next;
    }

    store
        .save_all()
        .await
        .map_err(|e| anyhow::anyhow!("save_all after seed: {}", e))?;

    info!(
        "[LoroModule] Seeded Loro with {} blocks from persistent store",
        applied
    );
    Ok(())
}

async fn apply_seed_row(
    backend: &LoroBackend,
    row: &std::collections::HashMap<String, Value>,
) -> anyhow::Result<bool> {
    let id = row
        .get("id")
        .and_then(|v| v.as_string())
        .ok_or_else(|| anyhow::anyhow!("row missing 'id'"))?
        .to_string();

    // Skip blocks already in Loro.
    if backend.resolve_to_tree_id(&id).await.is_some() {
        return Ok(false);
    }

    let parent_id_raw = row
        .get("parent_id")
        .and_then(|v| v.as_string())
        .unwrap_or("sentinel:no_parent")
        .to_string();

    // Resolve parent: look up in Loro by stable ID. If not present, and
    // the parent isn't the sentinel/no-parent, create a placeholder
    // root so the child has a home.
    let parent_uri = if backend.resolve_to_tree_id(&parent_id_raw).await.is_some() {
        EntityUri::from_raw(&parent_id_raw)
    } else {
        let parent_entity = EntityUri::from_raw(&parent_id_raw);
        if parent_entity.is_no_parent() || parent_entity.is_sentinel() {
            parent_entity
        } else {
            let placeholder_uri = backend
                .create_placeholder_root(parent_entity.id())
                .await
                .map_err(|e| anyhow::anyhow!("create placeholder: {}", e))?;
            EntityUri::from_raw(&placeholder_uri)
        }
    };

    let content_str = row
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .to_string();
    let content_type_str = row
        .get("content_type")
        .and_then(|v| v.as_string())
        .unwrap_or("text");
    let content = if content_type_str == "source" {
        let lang = row
            .get("source_language")
            .and_then(|v| v.as_string())
            .unwrap_or("text");
        BlockContent::source(lang, content_str)
    } else {
        BlockContent::text(content_str)
    };

    let block_id_uri = EntityUri::from_raw(&id);
    let created = backend
        .create_block(parent_uri, content, Some(block_id_uri))
        .await
        .map_err(|e| anyhow::anyhow!("create_block for {}: {}", id, e))?;

    // Hydrate tags onto the freshly-created block. The literal `"Page"` tag
    // marks the block as a page (formerly the `name`-bearing variant). The
    // `tags` column is `#[jsonb]` and CDC seeds deliver it as Value::Array
    // when Turso parses the column, or Value::Json/Value::String when the
    // raw TEXT is passed through. Any other shape is a programming error.
    let tags = parse_seed_row_tags(row)?;
    if !tags.is_empty() {
        backend
            .set_block_tags(created.id.as_str(), &tags)
            .await
            .map_err(|e| anyhow::anyhow!("set_block_tags: {}", e))?;
    }

    // Properties: stored as JSON in the `properties` jsonb column. Same
    // shape variance as `tags`: parse the JSON list explicitly instead of
    // assuming Value::String.
    if let Some(map) = parse_seed_row_properties(row)? {
        if !map.is_empty() {
            let props: std::collections::HashMap<String, Value> = map
                .into_iter()
                .map(|(k, v)| {
                    let val = match v {
                        serde_json::Value::String(s) => Value::String(s),
                        serde_json::Value::Number(n) => {
                            if let Some(i) = n.as_i64() {
                                Value::Integer(i)
                            } else {
                                Value::Float(n.as_f64().unwrap_or(0.0))
                            }
                        }
                        serde_json::Value::Bool(b) => Value::Boolean(b),
                        serde_json::Value::Null => Value::Null,
                        _ => Value::String(v.to_string()),
                    };
                    (k, val)
                })
                .collect();
            backend
                .update_block_properties(created.id.as_str(), &props)
                .await
                .map_err(|e| anyhow::anyhow!("update_block_properties: {}", e))?;
        }
    }

    // Unused-import shim (ContentType is re-exported for parity with other seeders).
    let _ = ContentType::Text;
    Ok(true)
}

/// Parse the `tags` field of a CDC seed row into a typed `Vec<String>`.
/// Accepts `Value::Array<String>`, `Value::Json` / `Value::String` containing a
/// JSON list, or `Value::Null`/absent (treated as empty). Any other shape —
/// non-string array elements, malformed JSON, unexpected variant — fails
/// loudly so a malformed peer payload can't silently drop tag data.
fn parse_seed_row_tags(
    row: &std::collections::HashMap<String, Value>,
) -> anyhow::Result<Vec<String>> {
    match row.get("tags") {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(arr)) => arr
            .iter()
            .map(|elem| match elem {
                Value::String(s) => Ok(s.clone()),
                other => Err(anyhow::anyhow!(
                    "[apply_seed_row] tag entry is not a string: {:?}",
                    other
                )),
            })
            .collect(),
        Some(Value::Json(s)) | Some(Value::String(s)) => serde_json::from_str::<Vec<String>>(s)
            .map_err(|e| {
                anyhow::anyhow!(
                    "[apply_seed_row] tags string is not a JSON list: {} (raw: {:?})",
                    e,
                    s
                )
            }),
        Some(other) => Err(anyhow::anyhow!(
            "[apply_seed_row] tags has unexpected shape: {:?}",
            other
        )),
    }
}

/// Parse the `properties` field of a CDC seed row into a JSON object.
/// Returns `Ok(None)` if the column is absent / Null. Fails loudly on
/// unexpected shapes so a malformed peer payload can't silently drop
/// properties.
fn parse_seed_row_properties(
    row: &std::collections::HashMap<String, Value>,
) -> anyhow::Result<Option<std::collections::HashMap<String, serde_json::Value>>> {
    let raw = match row.get("properties") {
        None | Some(Value::Null) => return Ok(None),
        Some(v) => v,
    };
    let parsed: std::collections::HashMap<String, serde_json::Value> = match raw {
        Value::Json(s) | Value::String(s) => serde_json::from_str(s).map_err(|e| {
            anyhow::anyhow!(
                "[apply_seed_row] properties string is not a JSON object: {} (raw: {:?})",
                e,
                s
            )
        })?,
        Value::Object(m) => m
            .iter()
            .map(|(k, v)| (k.clone(), v.clone().into()))
            .collect(),
        other => {
            return Err(anyhow::anyhow!(
                "[apply_seed_row] properties has unexpected shape: {:?}",
                other
            ));
        }
    };
    Ok(Some(parsed))
}

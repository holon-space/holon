use std::collections::HashMap;

use anyhow::Result;
use fluxdi::{Injector, Module, Provider, Shared};

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::api::backend_engine::BackendEngine;
use crate::api::operation_dispatcher::{OperationDispatcher, OperationModule};
use crate::core::datasource::{OperationObserver, OperationProvider, SyncTokenStore};
use crate::core::operation_log::{OperationLogObserver, OperationLogStore};
use crate::entity_profile::{LiveEntities, ProfileResolver, parse_entity_profile};
use crate::identity::IdentityProvider;
use crate::navigation::NavigationProvider;
use crate::storage::graph_schema::GraphSchemaRegistry;
use crate::storage::schema_module::SchemaModule;
use crate::storage::schema_modules::{BlockSchemaModule, LinkSchemaModule, NavigationSchemaModule};
use crate::storage::sync_token_store::DatabaseSyncTokenStore;
use crate::storage::turso::{DbHandle, TursoBackend};
use crate::storage::{ChangeOriginInjector, JsonAggregationSqlTransformer, SqlTransformer};
use crate::sync::LiveData;
use crate::type_registry::{TypeRegistry, create_default_registry};

use super::schema_providers::{
    BlockHierarchyView, CoreTables, DbReady, GraphEavSchema, IdentityTables, LinkTables,
    NavigationTables, OperationTables, SyncStateTables, register_schema_providers,
};
use super::{
    DatabasePathConfig, DbHandleProvider, DbHandleProviderImpl, TursoBackendProvider,
    TursoBackendProviderImpl,
};

use super::lifecycle::preload_startup_views;

/// Build the default set of SQL-level transformers (applied after compilation).
fn build_sql_transformers() -> Vec<Box<dyn SqlTransformer>> {
    let mut transformers: Vec<Box<dyn SqlTransformer>> = vec![
        Box::new(ChangeOriginInjector),
        Box::new(JsonAggregationSqlTransformer),
    ];
    transformers.sort_by_key(|t| t.priority());
    transformers
}

/// Initialize an OperationLogStore with its schema.
async fn init_operation_log_store(db_handle: DbHandle) -> OperationLogStore {
    let store = OperationLogStore::new(db_handle.clone());
    store
        .initialize_schema()
        .await
        .expect("Failed to initialize operations table");
    OperationLogStore::new(db_handle)
}

/// Initialize a SyncTokenStore with its schema.
async fn init_sync_token_store(db_handle: DbHandle) -> Arc<dyn SyncTokenStore> {
    let store = DatabaseSyncTokenStore::new(db_handle)
        .await
        .expect("Failed to initialize sync_states table");
    Arc::new(store)
}

/// Build a populated GraphSchemaRegistry from the TypeRegistry + module contributions.
fn build_graph_schema_registry(type_registry: &TypeRegistry) -> GraphSchemaRegistry {
    let mut registry = GraphSchemaRegistry::new();

    for type_def in type_registry.all() {
        registry.register_type(type_def);
    }

    let (nodes, edges) = NavigationSchemaModule.graph_contributions();
    registry.register_nodes(nodes);
    registry.register_edges(edges);

    let (nodes, edges) = LinkSchemaModule.graph_contributions();
    registry.register_nodes(nodes);
    registry.register_edges(edges);

    registry.register_edge_fields(BlockSchemaModule.edge_fields());

    registry
}

/// Create and initialize a BackendEngine from a backend, dispatcher, and config.
///
/// Schema initialization is handled by `resolve_all_eager()` in the lifecycle
/// layer (called before BackendEngine resolution). The `DbReady<*>` markers
/// are already cached by the time this factory runs.
async fn create_initialized_engine(
    backend: Arc<RwLock<TursoBackend>>,
    dispatcher: Arc<OperationDispatcher>,
    ui_info: holon_api::UiInfo,
    graph_schema_registry: GraphSchemaRegistry,
    type_registry: &TypeRegistry,
) -> BackendEngine {
    let backend_guard = backend.read().await;
    let db_handle = backend_guard.handle().clone();
    drop(backend_guard);

    // Build type-defined profiles from TypeRegistry. `profile_from_type_def`
    // can't see `virtual_child` because it's stored in a side map on the
    // registry (TypeDefinition lives in holon-api, VirtualChildConfig in
    // holon — keeping them split avoids a cross-crate dep flip), so we
    // attach it here.
    let type_profiles: Vec<_> = type_registry
        .all()
        .iter()
        .filter_map(|td| {
            crate::entity_profile::profile_from_type_def(td).map(|mut p| {
                p.virtual_child = type_registry.virtual_child_config(&td.name);
                p
            })
        })
        .collect();

    let ddl_mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));
    let matview_mgr = crate::sync::MatviewManager::new(db_handle.clone(), ddl_mutex.clone());

    // Now the block table exists — create profile resolver with CDC.
    let profile_resolver = create_profile_resolver(
        &matview_mgr,
        &dispatcher,
        ui_info,
        LiveEntities::new(),
        type_profiles,
    )
    .await;

    let engine = BackendEngine::new(
        db_handle,
        dispatcher,
        profile_resolver.clone(),
        build_sql_transformers(),
        graph_schema_registry,
    )
    .expect("Failed to create BackendEngine");

    // Preload startup matviews (reuses existing ones from previous sessions).
    preload_startup_views(&engine, None)
        .await
        .expect("Failed to preload startup views");

    let live_entities = create_live_entities(&matview_mgr).await;
    profile_resolver.set_live_entities(live_entities);

    engine
}

/// Register services shared between `register_core_services` and
/// `register_core_services_with_backend`: TypeRegistry, OperationObserver,
/// NavigationProvider, OperationProvider (nav), OperationModule.
fn register_shared_services(injector: &Injector) -> Result<()> {
    let type_registry = create_default_registry().expect("Failed to create default TypeRegistry");
    injector.provide::<TypeRegistry>(Provider::root(move |_| type_registry.clone()));

    injector.provide_into_set::<dyn OperationObserver>(Provider::root_async(
        move |inj| async move {
            let store = inj.resolve_async::<OperationLogStore>().await;
            Arc::new(OperationLogObserver::new(store)) as Arc<dyn OperationObserver>
        },
    ));

    injector.provide_into_set::<dyn OperationProvider>(Provider::root(|inj| {
        let nav_provider = inj.resolve::<NavigationProvider>();
        nav_provider as Arc<dyn OperationProvider>
    }));

    injector.provide_into_set::<dyn OperationProvider>(Provider::root(|inj| {
        let identity_provider = inj.resolve::<IdentityProvider>();
        identity_provider as Arc<dyn OperationProvider>
    }));

    OperationModule
        .configure(injector)
        .map_err(|e| anyhow::anyhow!("Failed to register OperationModule: {}", e))?;

    Ok(())
}

/// Register core services in the DI container.
pub fn register_core_services(injector: &Injector, db_path: PathBuf) -> Result<()> {
    tracing::debug!(
        "[DI] register_core_services called with db_path: {:?}",
        db_path
    );

    let db_path_for_config = db_path.clone();
    injector.provide::<DatabasePathConfig>(Provider::root(move |_| {
        Shared::new(DatabasePathConfig::new(db_path_for_config.clone()))
    }));
    tracing::debug!("[DI] Registered DatabasePathConfig");

    tracing::debug!("[DI] Registering RwLock<TursoBackend> factory");
    let db_path_clone = db_path.clone();
    injector.provide::<RwLock<TursoBackend>>(Provider::root(move |_| {
        tracing::debug!("[DI] RwLock<TursoBackend> factory called");
        let db = TursoBackend::open_database(&db_path_clone).expect("Failed to open database");
        let (cdc_tx, _) = tokio::sync::broadcast::channel(1024);
        let (backend, _db_handle) =
            TursoBackend::new(db, cdc_tx).expect("Failed to create TursoBackend");
        tracing::debug!("[DI] TursoBackend created successfully, wrapping in RwLock");
        Shared::new(RwLock::new(backend))
    }));

    injector.provide::<dyn TursoBackendProvider>(Provider::root(|inj| {
        let backend = inj.resolve::<RwLock<TursoBackend>>();
        Arc::new(TursoBackendProviderImpl {
            backend: backend.clone(),
        }) as Arc<dyn TursoBackendProvider>
    }));

    injector.provide::<dyn DbHandleProvider>(Provider::root(|inj| {
        let backend = inj.resolve::<RwLock<TursoBackend>>();
        // Native — full multi-thread runtime, can block_in_place to descend into async.
        #[cfg(not(target_arch = "wasm32"))]
        let db_handle = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { backend.read().await.handle() })
        });
        // wasi-threads (holon-worker) — current-thread runtime, no block_in_place.
        // The worker initializes BackendEngine once and keeps it; callers must
        // resolve via root_async or be inside the same single thread.
        #[cfg(all(target_arch = "wasm32", target_os = "wasi"))]
        let db_handle = {
            let _ = backend;
            panic!("DbHandleProvider sync factory unsupported on wasi; use root_async")
        };
        // wasm32-unknown (dioxus-web) — no tokio runtime at all.
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        let db_handle = {
            let _ = backend;
            panic!("DbHandleProvider sync factory unsupported on wasm32; use root_async")
        };
        Arc::new(DbHandleProviderImpl { handle: db_handle }) as Arc<dyn DbHandleProvider>
    }));

    injector.provide::<dyn SyncTokenStore>(
        Provider::root_async(move |inj| async move {
            let _sync = inj.resolve_async::<DbReady<SyncStateTables>>().await;
            let db_handle_provider = inj.resolve::<dyn DbHandleProvider>();
            init_sync_token_store(db_handle_provider.handle()).await
        })
        .with_dependency::<DbReady<SyncStateTables>>(),
    );

    injector.provide::<OperationLogStore>(
        Provider::root_async(move |inj| async move {
            let _ops = inj.resolve_async::<DbReady<OperationTables>>().await;
            let db_handle_provider = inj.resolve::<dyn DbHandleProvider>();
            Shared::new(init_operation_log_store(db_handle_provider.handle()).await)
        })
        .with_dependency::<DbReady<OperationTables>>(),
    );

    injector.provide::<NavigationProvider>(
        Provider::root(move |inj| {
            let db_handle_provider = inj.resolve::<dyn DbHandleProvider>();
            Shared::new(NavigationProvider::new(db_handle_provider.handle()))
        })
        .with_dependency::<DbReady<NavigationTables>>(),
    );

    injector.provide::<IdentityProvider>(
        Provider::root(move |inj| {
            let db_handle_provider = inj.resolve::<dyn DbHandleProvider>();
            Shared::new(IdentityProvider::new(db_handle_provider.handle()))
        })
        .with_dependency::<DbReady<IdentityTables>>(),
    );

    register_shared_services(injector)?;
    register_schema_providers(injector);

    injector.provide::<BackendEngine>(
        Provider::root_async(move |inj| async move {
            tracing::debug!("[DI] BackendEngine factory called");

            // Eagerly resolve all schema deps in parallel waves.
            // FluxDI topologically sorts these roots and their transitive deps,
            // running independent schemas (CoreTables, NavigationTables, etc.)
            // concurrently in wave 0, then dependent ones in wave 1.
            inj.resolve_eager_roots(&super::schema_providers::all_schema_roots())
                .await
                .expect("Schema initialization failed");

            let backend = inj.resolve::<RwLock<TursoBackend>>().clone();
            let dispatcher = inj.resolve_async::<OperationDispatcher>().await;
            let ui_info: holon_api::UiInfo = inj
                .try_resolve::<holon_api::UiInfo>()
                .map(|a| (*a).clone())
                .unwrap_or_else(|_| holon_api::UiInfo::permissive());

            let type_registry = inj.resolve::<TypeRegistry>();
            let graph_schema_registry = build_graph_schema_registry(&type_registry);

            Shared::new(
                create_initialized_engine(
                    backend,
                    dispatcher,
                    ui_info,
                    graph_schema_registry,
                    &type_registry,
                )
                .await,
            )
        })
        .with_dependency::<DbReady<CoreTables>>()
        .with_dependency::<DbReady<BlockHierarchyView>>()
        .with_dependency::<DbReady<NavigationTables>>()
        .with_dependency::<DbReady<SyncStateTables>>()
        .with_dependency::<DbReady<OperationTables>>()
        .with_dependency::<DbReady<LinkTables>>()
        .with_dependency::<DbReady<IdentityTables>>()
        .with_dependency::<DbReady<GraphEavSchema>>(),
    );

    Ok(())
}

const PROFILE_SQL: &str = include_str!("../../sql/profiles/get_profiles.sql");
fn query_source_blocks_sql() -> String {
    format!(
        "SELECT id, parent_id, source_language FROM block \
         WHERE content_type = 'source' AND source_language IN {}",
        holon_api::QueryLanguage::sql_in_list()
    )
}

/// Create a CDC-driven LiveData<StorageEntity> from a SQL query, keyed by a given column.
async fn create_live_data_keyed_by(
    matview_manager: &crate::sync::MatviewManager,
    sql: &str,
    key_column: &'static str,
) -> Option<Arc<LiveData<crate::storage::types::StorageEntity>>> {
    match matview_manager.watch(sql).await {
        Ok(result) => {
            let live = LiveData::new(
                result.initial_rows,
                move |row| {
                    let id = row
                        .get(key_column)
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("entity row missing '{key_column}'"))?;
                    Ok(id)
                },
                |row| Ok(row.clone()),
            );
            live.subscribe(result.stream);
            Some(live)
        }
        Err(e) => {
            tracing::warn!("[DI] Failed to create live data for '{sql}': {e}");
            None
        }
    }
}

/// Build the `live_entities` map for ProfileResolver's Rhai entity lookups.
async fn create_live_entities(
    matview_manager: &crate::sync::MatviewManager,
) -> crate::entity_profile::LiveEntities {
    let mut live_entities = std::collections::HashMap::new();
    let qs_sql = query_source_blocks_sql();
    if let Some(qs) = create_live_data_keyed_by(matview_manager, &qs_sql, "parent_id").await {
        live_entities.insert(holon_api::EntityName::new("query_source"), qs);
    }
    live_entities
}

/// Create a CDC-driven ProfileResolver via MatviewManager + LiveData.
async fn create_profile_resolver(
    matview_manager: &crate::sync::MatviewManager,
    dispatcher: &Arc<OperationDispatcher>,
    ui_info: holon_api::UiInfo,
    live_entities: LiveEntities,
    type_profiles: Vec<crate::entity_profile::EntityProfile>,
) -> Arc<ProfileResolver> {
    use holon_api::EntityName;
    let mut entity_operations: HashMap<EntityName, Vec<holon_api::OperationDescriptor>> =
        HashMap::new();
    for op in dispatcher.operations() {
        entity_operations
            .entry(op.entity_name.clone())
            .or_default()
            .push(op);
    }
    match matview_manager.watch(PROFILE_SQL).await {
        Ok(result) => {
            let live_profiles = LiveData::new(
                result.initial_rows,
                |row| {
                    let id = row
                        .get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s.to_string())
                        .ok_or_else(|| anyhow::anyhow!("profile row missing 'id'"))?;
                    Ok(id)
                },
                |row| {
                    let content = row
                        .get("content")
                        .and_then(|v| v.as_string())
                        .ok_or_else(|| anyhow::anyhow!("profile row missing 'content'"))?;
                    parse_entity_profile(content)
                },
            );
            live_profiles.subscribe(result.stream);
            Arc::new(ProfileResolver::with_type_profiles(
                live_profiles,
                ui_info,
                live_entities,
                entity_operations,
                type_profiles,
            ))
        }
        Err(e) => {
            tracing::debug!(
                "[DI] ⚠️ Failed to set up profile watcher: {e:#}. Using empty profiles."
            );
            tracing::warn!("[DI] Failed to set up profile watcher: {e}. Using empty profiles.");
            let live_profiles: Arc<LiveData<crate::entity_profile::EntityProfile>> = LiveData::new(
                vec![],
                |_| Ok(String::new()),
                |_| anyhow::bail!("no profiles"),
            );
            Arc::new(ProfileResolver::with_type_profiles(
                live_profiles,
                ui_info,
                live_entities,
                entity_operations,
                type_profiles,
            ))
        }
    }
}

/// Register core services with a pre-created TursoBackend and DbHandle.
pub fn register_core_services_with_backend(
    injector: &Injector,
    db_path: PathBuf,
    backend: Arc<RwLock<TursoBackend>>,
    db_handle: DbHandle,
) -> Result<()> {
    tracing::debug!(
        "[DI] register_core_services_with_backend called with db_path: {:?}",
        db_path
    );

    injector.provide::<DatabasePathConfig>(Provider::root(move |_| {
        Shared::new(DatabasePathConfig::new(db_path.clone()))
    }));
    tracing::debug!("[DI] Registered DatabasePathConfig");

    let backend_for_provider = backend.clone();
    injector.provide::<dyn TursoBackendProvider>(Provider::root(move |_| {
        Arc::new(TursoBackendProviderImpl {
            backend: backend_for_provider.clone(),
        }) as Arc<dyn TursoBackendProvider>
    }));

    let db_handle_for_sync = db_handle.clone();
    let db_handle_for_log = db_handle.clone();
    let db_handle_for_nav = db_handle.clone();
    let db_handle_for_identity = db_handle.clone();

    injector.provide::<dyn DbHandleProvider>(Provider::root(move |_| {
        tracing::debug!("[DI] Registering pre-created DbHandle");
        Arc::new(DbHandleProviderImpl {
            handle: db_handle.clone(),
        }) as Arc<dyn DbHandleProvider>
    }));

    injector.provide::<dyn SyncTokenStore>(
        Provider::root_async(move |inj| {
            let h = db_handle_for_sync.clone();
            async move {
                let _sync = inj.resolve_async::<DbReady<SyncStateTables>>().await;
                init_sync_token_store(h).await
            }
        })
        .with_dependency::<DbReady<SyncStateTables>>(),
    );

    injector.provide::<OperationLogStore>(
        Provider::root_async(move |inj| {
            let h = db_handle_for_log.clone();
            async move {
                let _ops = inj.resolve_async::<DbReady<OperationTables>>().await;
                Shared::new(init_operation_log_store(h).await)
            }
        })
        .with_dependency::<DbReady<OperationTables>>(),
    );

    injector.provide::<NavigationProvider>(
        Provider::root(move |_| Shared::new(NavigationProvider::new(db_handle_for_nav.clone())))
            .with_dependency::<DbReady<NavigationTables>>(),
    );

    injector.provide::<IdentityProvider>(
        Provider::root(move |_| Shared::new(IdentityProvider::new(db_handle_for_identity.clone())))
            .with_dependency::<DbReady<IdentityTables>>(),
    );

    injector.provide::<crate::sync::MatviewManager>(Provider::root(|inj| {
        let db_handle_provider = inj.resolve::<dyn DbHandleProvider>();
        let ddl_mutex = std::sync::Arc::new(tokio::sync::Mutex::new(()));
        Shared::new(crate::sync::MatviewManager::new(
            db_handle_provider.handle(),
            ddl_mutex,
        ))
    }));

    register_shared_services(injector)?;
    register_schema_providers(injector);

    let backend_for_engine = backend.clone();
    injector.provide::<BackendEngine>(
        Provider::root_async(move |inj| {
            let backend = backend_for_engine.clone();
            async move {
                tracing::debug!("[DI] BackendEngine factory called (with pre-created backend)");

                inj.resolve_eager_roots(&super::schema_providers::all_schema_roots())
                    .await
                    .expect("Schema initialization failed");

                let dispatcher = inj.resolve_async::<OperationDispatcher>().await;
                let ui_info: holon_api::UiInfo = inj
                    .try_resolve::<holon_api::UiInfo>()
                    .map(|a| (*a).clone())
                    .unwrap_or_else(|_| holon_api::UiInfo::permissive());

                let type_registry = inj.resolve::<TypeRegistry>();
                let graph_schema_registry = build_graph_schema_registry(&type_registry);

                Shared::new(
                    create_initialized_engine(
                        backend,
                        dispatcher,
                        ui_info,
                        graph_schema_registry,
                        &type_registry,
                    )
                    .await,
                )
            }
        })
        .with_dependency::<DbReady<CoreTables>>()
        .with_dependency::<DbReady<BlockHierarchyView>>()
        .with_dependency::<DbReady<NavigationTables>>()
        .with_dependency::<DbReady<SyncStateTables>>()
        .with_dependency::<DbReady<OperationTables>>()
        .with_dependency::<DbReady<LinkTables>>()
        .with_dependency::<DbReady<IdentityTables>>()
        .with_dependency::<DbReady<GraphEavSchema>>(),
    );

    Ok(())
}

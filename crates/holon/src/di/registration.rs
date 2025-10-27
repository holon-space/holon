//! DI service registration functions.

use std::collections::HashMap;

use anyhow::Result;
use ferrous_di::{Lifetime, Resolver, ServiceCollection, ServiceCollectionModuleExt};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::api::backend_engine::BackendEngine;
use crate::api::operation_dispatcher::{OperationDispatcher, OperationModule};
use crate::core::datasource::{
    EntitySchemaProvider, OperationObserver, OperationProvider, SyncTokenStore,
};
use crate::core::operation_log::{OperationLogObserver, OperationLogStore};
use crate::entity_profile::{ProfileResolver, parse_entity_profile};
use crate::navigation::NavigationProvider;
use crate::storage::graph_schema::GraphSchemaRegistry;
use crate::storage::schema_module::SchemaModule;
use crate::storage::schema_modules::NavigationSchemaModule;
use crate::storage::sync_token_store::DatabaseSyncTokenStore;
use crate::storage::turso::{DbHandle, TursoBackend};
use crate::storage::{ChangeOriginInjector, JsonAggregationSqlTransformer, SqlTransformer};
use crate::sync::LiveData;

use super::runtime::run_async_in_sync_factory;
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
fn init_operation_log_store(db_handle: DbHandle) -> OperationLogStore {
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            let store = OperationLogStore::new(db_handle.clone());
            store
                .initialize_schema()
                .await
                .expect("Failed to initialize operations table");
        });
    });

    OperationLogStore::new(db_handle)
}

/// Initialize a SyncTokenStore with its schema.
fn init_sync_token_store(db_handle: DbHandle) -> Arc<dyn SyncTokenStore> {
    let store = tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            DatabaseSyncTokenStore::new(db_handle)
                .await
                .expect("Failed to initialize sync_states table")
        })
    });

    Arc::new(store)
}

/// Build a populated GraphSchemaRegistry from all registered EntitySchemaProviders + module contributions.
///
/// The `providers` argument is collected via `resolver.get_all_trait::<dyn EntitySchemaProvider>()`
/// in the calling factory (since `Resolver` is not dyn-compatible).
///
/// Returns the registry (not a built schema) so BackendEngine can hold it
/// for runtime entity additions (MCP).
fn build_graph_schema_registry(providers: &[Arc<dyn EntitySchemaProvider>]) -> GraphSchemaRegistry {
    let mut registry = GraphSchemaRegistry::new();

    for provider in providers {
        for schema in provider.entity_schemas() {
            registry.register_entity(schema);
        }
    }

    let (nodes, edges) = NavigationSchemaModule.graph_contributions();
    registry.register_nodes(nodes);
    registry.register_edges(edges);

    registry
}

/// Create and initialize a BackendEngine from a backend, dispatcher, and config.
///
/// This is the shared async logic used by both registration functions.
async fn create_initialized_engine(
    backend: Arc<RwLock<TursoBackend>>,
    dispatcher: Arc<OperationDispatcher>,
    db_path: PathBuf,
    ui_info: holon_api::UiInfo,
    graph_schema_registry: GraphSchemaRegistry,
) -> BackendEngine {
    let backend_guard = backend.read().await;
    let db_handle = backend_guard.handle().clone();
    drop(backend_guard);

    let matview_mgr = crate::sync::MatviewManager::new(
        db_handle.clone(),
        std::sync::Arc::new(tokio::sync::Mutex::new(())),
    );
    let profile_resolver = create_profile_resolver(&matview_mgr, &dispatcher, ui_info).await;

    let engine = BackendEngine::new(
        db_handle,
        dispatcher,
        profile_resolver,
        build_sql_transformers(),
        graph_schema_registry,
    )
    .expect("Failed to create BackendEngine");

    engine
        .blocks()
        .initialize_database_if_needed(&db_path)
        .await
        .expect("Failed to initialize database");

    preload_startup_views(&engine, None)
        .await
        .expect("Failed to preload startup views");

    engine
}

/// Core entity schema provider for Block and Document.
struct CoreEntitySchemaProvider;

impl EntitySchemaProvider for CoreEntitySchemaProvider {
    fn entity_schemas(&self) -> Vec<holon_api::EntitySchema> {
        vec![
            holon_api::Block::entity_schema(),
            holon_api::Document::entity_schema(),
        ]
    }
}

/// Register services shared between `register_core_services` and
/// `register_core_services_with_backend`: OperationObserver, NavigationProvider,
/// OperationProvider (nav), OperationModule.
fn register_shared_services(services: &mut ServiceCollection) -> Result<()> {
    services.add_trait_factory::<dyn EntitySchemaProvider, _>(Lifetime::Singleton, |_| {
        Arc::new(CoreEntitySchemaProvider) as Arc<dyn EntitySchemaProvider>
    });

    services.add_trait_factory::<dyn OperationObserver, _>(Lifetime::Singleton, move |resolver| {
        let store = resolver.get_required::<OperationLogStore>();
        Arc::new(OperationLogObserver::new(store)) as Arc<dyn OperationObserver>
    });

    services.add_trait_factory::<dyn OperationProvider, _>(Lifetime::Singleton, |resolver| {
        let nav_provider = resolver.get_required::<NavigationProvider>();
        nav_provider as Arc<dyn OperationProvider>
    });

    services
        .add_module_mut(OperationModule)
        .map_err(|e| anyhow::anyhow!("Failed to register OperationModule: {}", e))?;

    Ok(())
}

/// Register core services in the DI container.
///
/// This registers:
/// - `DatabasePathConfig` (singleton) - Database path configuration
/// - `RwLock<TursoBackend>` (singleton) - Database backend
/// - `OperationDispatcher` (singleton) - Operation dispatcher
/// - `BackendEngine` (singleton) - Render engine
pub fn register_core_services(services: &mut ServiceCollection, db_path: PathBuf) -> Result<()> {
    eprintln!(
        "[DI] register_core_services called with db_path: {:?}",
        db_path
    );

    services.add_singleton(DatabasePathConfig::new(db_path.clone()));
    eprintln!("[DI] Registered DatabasePathConfig");

    eprintln!("[DI] Registering RwLock<TursoBackend> factory");
    let db_path_clone = db_path.clone();
    services.add_singleton_factory::<RwLock<TursoBackend>, _>(move |_resolver| {
        eprintln!("[DI] RwLock<TursoBackend> factory called - about to spawn thread");
        #[cfg(not(target_arch = "wasm32"))]
        {
            let db_path_for_thread = db_path_clone.clone();
            eprintln!("[DI] Spawning thread to create TursoBackend");
            let backend = std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
                rt.block_on(async {
                    let db = TursoBackend::open_database(&db_path_for_thread)
                        .expect("Failed to open database");
                    let (cdc_tx, _) = tokio::sync::broadcast::channel(1024);
                    let (backend, _db_handle) =
                        TursoBackend::new(db, cdc_tx).expect("Failed to create TursoBackend");
                    backend
                })
            })
            .join()
            .expect("Thread panicked while creating TursoBackend");
            eprintln!("[DI] TursoBackend created successfully, wrapping in RwLock");
            RwLock::new(backend)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let rt = tokio::runtime::Handle::current();
            let backend = rt.block_on(async {
                let db =
                    TursoBackend::open_database(&db_path_clone).expect("Failed to open database");
                let (cdc_tx, _) = tokio::sync::broadcast::channel(1024);
                let (backend, _db_handle) =
                    TursoBackend::new(db, cdc_tx).expect("Failed to create TursoBackend");
                backend
            });
            RwLock::new(backend)
        }
    });

    services.add_trait_factory::<dyn TursoBackendProvider, _>(Lifetime::Singleton, |resolver| {
        let backend = resolver.get_required::<RwLock<TursoBackend>>();
        Arc::new(TursoBackendProviderImpl {
            backend: backend.clone(),
        }) as Arc<dyn TursoBackendProvider>
    });

    services.add_trait_factory::<dyn DbHandleProvider, _>(Lifetime::Singleton, |resolver| {
        let backend = resolver.get_required::<RwLock<TursoBackend>>();
        let db_handle = run_async_in_sync_factory(async move { backend.read().await.handle() });
        Arc::new(DbHandleProviderImpl { handle: db_handle }) as Arc<dyn DbHandleProvider>
    });

    services.add_trait_factory::<dyn SyncTokenStore, _>(Lifetime::Singleton, move |resolver| {
        let db_handle_provider = resolver.get_required_trait::<dyn DbHandleProvider>();
        init_sync_token_store(db_handle_provider.handle())
    });

    services.add_singleton_factory::<OperationLogStore, _>(move |resolver| {
        let db_handle_provider = resolver.get_required_trait::<dyn DbHandleProvider>();
        init_operation_log_store(db_handle_provider.handle())
    });

    services.add_singleton_factory::<NavigationProvider, _>(move |resolver| {
        let db_handle_provider = resolver.get_required_trait::<dyn DbHandleProvider>();
        NavigationProvider::new(db_handle_provider.handle())
    });

    register_shared_services(services)?;

    services.add_singleton_factory::<BackendEngine, _>(move |resolver| {
        eprintln!("[DI] BackendEngine factory called");
        let backend = resolver.get_required::<RwLock<TursoBackend>>().clone();
        let dispatcher = resolver.get_required::<OperationDispatcher>();
        let db_path_config: Arc<DatabasePathConfig> = resolver.get_required::<DatabasePathConfig>();
        let ui_info: holon_api::UiInfo = resolver
            .get::<holon_api::UiInfo>()
            .map(|a| (*a).clone())
            .unwrap_or_else(|_| holon_api::UiInfo::permissive());

        let schema_providers = resolver
            .get_all_trait::<dyn EntitySchemaProvider>()
            .unwrap_or_else(|_| vec![]);
        let graph_schema_registry = build_graph_schema_registry(&schema_providers);

        run_async_in_sync_factory(create_initialized_engine(
            backend,
            dispatcher,
            db_path_config.path.clone(),
            ui_info,
            graph_schema_registry,
        ))
    });

    Ok(())
}

const PROFILE_SQL: &str = include_str!("../../sql/profiles/get_profiles.sql");
const DOCUMENTS_SQL: &str = "SELECT * FROM document";

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
            eprintln!("[DI] Warning: Failed to create live data for '{sql}': {e}");
            None
        }
    }
}

/// Create a CDC-driven LiveData<StorageEntity> from a SQL query, keyed by `id`.
async fn create_live_data_for_entity(
    matview_manager: &crate::sync::MatviewManager,
    sql: &str,
) -> Option<Arc<LiveData<crate::storage::types::StorageEntity>>> {
    create_live_data_keyed_by(matview_manager, sql, "id").await
}

/// Build the `live_entities` map for ProfileResolver's Rhai entity lookups.
async fn create_live_entities(
    matview_manager: &crate::sync::MatviewManager,
) -> crate::entity_profile::LiveEntities {
    let mut live_entities = std::collections::HashMap::new();
    if let Some(docs) = create_live_data_for_entity(matview_manager, DOCUMENTS_SQL).await {
        live_entities.insert(holon_api::EntityName::new("document"), docs);
    }
    // Query source blocks keyed by parent_id — enables profile lookup:
    // `query_source(id)` returns the source block for a given parent heading.
    let qs_sql = query_source_blocks_sql();
    if let Some(qs) = create_live_data_keyed_by(matview_manager, &qs_sql, "parent_id").await {
        live_entities.insert(holon_api::EntityName::new("query_source"), qs);
    }
    live_entities
}

/// Create a CDC-driven ProfileResolver via MatviewManager + LiveData.
///
/// Profile blocks are queried from a materialized view. Changes (edits to profile
/// blocks in org files) are applied incrementally via CDC, so profile changes
/// take effect without an app restart.
async fn create_profile_resolver(
    matview_manager: &crate::sync::MatviewManager,
    dispatcher: &Arc<OperationDispatcher>,
    ui_info: holon_api::UiInfo,
) -> Arc<dyn crate::entity_profile::ProfileResolving> {
    let live_entities = create_live_entities(matview_manager).await;

    // Build entity operations map from the dispatcher — single source of truth.
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
            Arc::new(ProfileResolver::new(
                live_profiles,
                ui_info,
                live_entities,
                entity_operations,
            ))
        }
        Err(e) => {
            eprintln!("[DI] Warning: Failed to set up profile watcher: {e}. Using empty profiles.");
            let live_profiles: Arc<LiveData<crate::entity_profile::EntityProfile>> = LiveData::new(
                vec![],
                |_| Ok(String::new()),
                |_| anyhow::bail!("no profiles"),
            );
            Arc::new(ProfileResolver::new(
                live_profiles,
                ui_info,
                live_entities,
                entity_operations,
            ))
        }
    }
}

/// Register core services with a pre-created TursoBackend and DbHandle.
///
/// This variant takes a pre-created backend and DbHandle instead of creating them in factories.
/// Use this to avoid TypeId mismatch issues when cross-crate code needs the backend.
pub fn register_core_services_with_backend(
    services: &mut ServiceCollection,
    db_path: PathBuf,
    backend: Arc<RwLock<TursoBackend>>,
    db_handle: DbHandle,
) -> Result<()> {
    eprintln!(
        "[DI] register_core_services_with_backend called with db_path: {:?}",
        db_path
    );

    services.add_singleton(DatabasePathConfig::new(db_path.clone()));
    eprintln!("[DI] Registered DatabasePathConfig");

    let backend_for_provider = backend.clone();
    services.add_trait_factory::<dyn TursoBackendProvider, _>(
        Lifetime::Singleton,
        move |_resolver| {
            Arc::new(TursoBackendProviderImpl {
                backend: backend_for_provider.clone(),
            }) as Arc<dyn TursoBackendProvider>
        },
    );

    let db_handle_for_sync = db_handle.clone();
    let db_handle_for_log = db_handle.clone();
    let db_handle_for_nav = db_handle.clone();

    {
        services.add_trait_factory::<dyn DbHandleProvider, _>(
            Lifetime::Singleton,
            move |_resolver| {
                eprintln!("[DI] Registering pre-created DbHandle");
                Arc::new(DbHandleProviderImpl {
                    handle: db_handle.clone(),
                }) as Arc<dyn DbHandleProvider>
            },
        );
    }
    services.add_trait_factory::<dyn SyncTokenStore, _>(Lifetime::Singleton, move |_resolver| {
        init_sync_token_store(db_handle_for_sync.clone())
    });

    services.add_singleton_factory::<OperationLogStore, _>(move |_resolver| {
        init_operation_log_store(db_handle_for_log.clone())
    });

    services.add_singleton_factory::<NavigationProvider, _>(move |_resolver| {
        NavigationProvider::new(db_handle_for_nav.clone())
    });

    register_shared_services(services)?;

    let backend_for_engine = backend.clone();
    services.add_singleton_factory::<BackendEngine, _>(move |resolver| {
        eprintln!("[DI] BackendEngine factory called (with pre-created backend)");
        let backend = backend_for_engine.clone();
        let dispatcher = resolver.get_required::<OperationDispatcher>();
        let db_path_config: Arc<DatabasePathConfig> = resolver.get_required::<DatabasePathConfig>();
        let ui_info: holon_api::UiInfo = resolver
            .get::<holon_api::UiInfo>()
            .map(|a| (*a).clone())
            .unwrap_or_else(|_| holon_api::UiInfo::permissive());

        let schema_providers = resolver
            .get_all_trait::<dyn EntitySchemaProvider>()
            .unwrap_or_else(|_| vec![]);
        let graph_schema_registry = build_graph_schema_registry(&schema_providers);

        run_async_in_sync_factory(create_initialized_engine(
            backend,
            dispatcher,
            db_path_config.path.clone(),
            ui_info,
            graph_schema_registry,
        ))
    });

    Ok(())
}

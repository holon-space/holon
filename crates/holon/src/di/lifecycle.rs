//! App lifecycle functions: creating and initializing BackendEngine via DI.

use anyhow::Result;
use fluxdi::{Injector, Module};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use crate::api::backend_engine::BackendEngine;
use crate::storage::turso::TursoBackend;

use super::STARTUP_QUERIES;
use super::registration::register_core_services_with_backend;

/// Pre-create materialized views for startup queries.
///
/// Call this during initialization, AFTER schema modules have been initialized via DI
/// but BEFORE file watching or data sync starts.
///
/// This function is idempotent - safe to call multiple times.
/// Views that already exist are skipped.
pub async fn preload_startup_views(
    engine: &BackendEngine,
    additional_queries: Option<&[&str]>,
) -> Result<()> {
    tracing::debug!(
        "[DI] preload_startup_views: starting with {} common queries",
        STARTUP_QUERIES.len()
    );

    let compiled: Vec<String> = STARTUP_QUERIES
        .iter()
        .map(|prql| {
            engine
                .compile_to_sql(prql, holon_api::QueryLanguage::HolonPrql)
                .unwrap_or_else(|e| {
                    panic!("Failed to compile startup PRQL query: {e}\nQuery: {prql}")
                })
        })
        .collect();
    let compiled_refs: Vec<&str> = compiled.iter().map(|s| s.as_str()).collect();
    engine.preload_views(&compiled_refs).await?;

    if let Some(queries) = additional_queries {
        tracing::debug!(
            "[DI] preload_startup_views: preloading {} additional queries",
            queries.len()
        );
        engine.preload_views(queries).await?;
    }

    tracing::debug!("[DI] preload_startup_views: completed");
    Ok(())
}

/// Open database, create backend, register core services on the injector.
///
/// This is the sync portion of DI setup — suitable for use in `Module::configure()`.
/// Schema initialization happens lazily via `DbReady<*>` providers.
pub fn open_and_register_core(injector: &Injector, db_path: PathBuf) -> Result<()> {
    tracing::debug!("[DI] Opening database at {:?}...", db_path);
    let db = TursoBackend::open_database(&db_path).expect("Failed to open database");
    tracing::debug!("[DI] Database opened successfully");

    let (cdc_tx, _) = tokio::sync::broadcast::channel(1024);

    tracing::debug!("[DI] Creating TursoBackend...");
    let (backend_inner, db_handle) =
        TursoBackend::new(db, cdc_tx).expect("Failed to create TursoBackend");
    tracing::debug!("[DI] TursoBackend created");

    let backend = Arc::new(RwLock::new(backend_inner));

    register_core_services_with_backend(injector, db_path, backend, db_handle)?;
    Ok(())
}

/// Reusable module for core infrastructure: database setup.
///
/// - `configure()`: opens the database and registers core services (sync)
///
/// Schema initialization happens lazily via `DbReady<*>` providers when
/// services that need tables are resolved.
///
/// Compose this into frontend-specific modules via explicit delegation
/// (not `imports()`, which creates child injector scopes).
pub struct CoreInfraModule {
    pub db_path: PathBuf,
}

impl Module for CoreInfraModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        open_and_register_core(injector, self.db_path.clone()).map_err(|e| {
            fluxdi::Error::module_lifecycle_failed("CoreInfraModule", "configure", &e.to_string())
        })
    }
}

/// Open the database, create the DI injector with core services.
///
/// Schema initialization is not a separate step — it happens lazily when
/// services resolve their `DbReady<*>` dependencies.
async fn build_di_container<F>(db_path: PathBuf, setup_fn: F) -> Result<Arc<Injector>>
where
    F: FnOnce(&Injector) -> Result<()>,
{
    let injector = Injector::root();

    open_and_register_core(&injector, db_path)?;

    setup_fn(&injector)?;

    tracing::debug!("[DI] Injector built successfully");

    Ok(Arc::new(injector))
}

/// Shared setup function for creating BackendEngine with DI.
///
/// Sets up the DI container and returns a BackendEngine. Can be used by both TUI and Flutter.
pub async fn create_backend_engine<F>(db_path: PathBuf, setup_fn: F) -> Result<Arc<BackendEngine>>
where
    F: FnOnce(&Injector) -> Result<()>,
{
    let (engine, ()) =
        create_backend_engine_with_extras(db_path, setup_fn, |_| async { () }).await?;
    Ok(engine)
}

/// Create a BackendEngine and resolve additional services from DI.
///
/// Like `create_backend_engine` but allows resolving additional services
/// from the DI container after the engine is created. The `extra_resolve`
/// closure is async so it can resolve `root_async` providers.
///
/// `BackendEngine` resolution and `extra_resolve` run concurrently when
/// they don't share dependencies (e.g. CacheEventSubscriber wiring is
/// independent of engine creation).
pub async fn create_backend_engine_with_extras<F, G, Fut, T>(
    db_path: PathBuf,
    setup_fn: F,
    extra_resolve: G,
) -> Result<(Arc<BackendEngine>, T)>
where
    F: FnOnce(&Injector) -> Result<()>,
    G: FnOnce(Arc<Injector>) -> Fut,
    Fut: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    let injector = build_di_container(db_path, setup_fn).await?;

    // Resolve BackendEngine FIRST, then extras. Resolving in parallel causes
    // a fluxdi TOCTOU race: both tasks call resolve_async::<BackendEngine>,
    // both get a cache miss, both run the factory, creating duplicate
    // OrgSyncControllers and event subscribers.
    tracing::debug!("[DI] Resolving BackendEngine...");
    let start = crate::util::MonotonicInstant::now();

    let engine_result = injector.resolve_async::<BackendEngine>().await;

    tracing::debug!("[DI] Resolving extras...");
    let extra_result = extra_resolve(injector).await;

    tracing::info!(
        "[DI] Bootstrap completed in {:.1}ms",
        start.elapsed().as_secs_f64() * 1000.0
    );

    Ok((engine_result, extra_result))
}

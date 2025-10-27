//! Test helpers for dependency injection
//!
//! Provides utilities for setting up DI containers in tests.

use anyhow::Result;
use fluxdi::{Injector, Module, Provider};

use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::TursoBackendProvider;
use super::lifecycle::{create_backend_engine, create_backend_engine_with_extras};
use crate::api::backend_engine::BackendEngine;
use crate::core::datasource::{OperationProvider, SyncableProvider};
use crate::storage::turso::TursoBackend;

/// Create a BackendEngine for testing using dependency injection
///
/// This sets up a complete DI container with all core services, using an in-memory database.
/// This ensures tests use the same setup as production code.
///
/// # Example
/// ```rust
/// #[tokio::test]
/// async fn test_something() {
///     let engine = create_test_engine().await.unwrap();
///     // Use engine for testing...
/// }
/// ```
pub async fn create_test_engine() -> Result<Arc<BackendEngine>> {
    create_test_engine_with_path(":memory:".into()).await
}

/// Create a BackendEngine for testing with a specific database path
///
/// This sets up a complete DI container with all core services.
/// Useful for tests that need a specific database path or want to persist data.
///
/// # Arguments
/// * `db_path` - Path to the database file (use ":memory:" for in-memory)
///
/// # Example
/// ```rust
/// #[tokio::test]
/// async fn test_with_persistence() {
///     let engine = create_test_engine_with_path("/tmp/test.db".into()).await.unwrap();
///     // Use engine for testing...
/// }
/// ```
pub async fn create_test_engine_with_path(db_path: PathBuf) -> Result<Arc<BackendEngine>> {
    create_backend_engine(db_path, |_| Ok(())).await
}

/// Create a test engine with custom providers
///
/// This allows tests to register additional providers before creating the engine.
/// The providers will be collected by OperationModule and included in the OperationDispatcher.
///
/// # Arguments
/// * `db_path` - Path to the database file (use ":memory:" for in-memory)
/// * `setup_fn` - Closure that receives the Injector for registering modules
///
/// # Example
/// ```rust
/// #[tokio::test]
/// async fn test_with_custom_provider() {
///     let engine = create_test_engine_with_setup(":memory:".into(), |services| {
///         services.add_singleton(MyModuleConfig::new());
///         services.add_module_mut(MyModule)?;
///         Ok(())
///     }).await.unwrap();
/// }
/// ```
pub async fn create_test_engine_with_setup<F>(
    db_path: PathBuf,
    setup_fn: F,
) -> Result<Arc<BackendEngine>>
where
    F: FnOnce(&Injector) -> Result<()>,
{
    create_backend_engine(db_path, setup_fn).await
}

/// Create a test engine with custom setup and resolve additional services from DI
pub async fn create_test_engine_with_extras<F, G, Fut, T>(
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
    create_backend_engine_with_extras(db_path, setup_fn, extra_resolve).await
}

/// Type alias for operation provider factory functions
pub type OperationProviderFactory =
    Box<dyn FnOnce(Arc<RwLock<TursoBackend>>) -> Arc<dyn OperationProvider> + Send>;

/// Type alias for syncable provider factory functions
pub type SyncableProviderFactory =
    Box<dyn FnOnce(Arc<RwLock<TursoBackend>>) -> Arc<dyn SyncableProvider> + Send>;

/// Test-specific ServiceModule for registering providers
///
/// This module makes it easy to register operation providers and syncable providers
/// for testing. It supports both direct provider instances and factory-based providers
/// that receive the backend at creation time.
///
/// # Example with factory (recommended for providers that need the backend)
/// ```rust
/// use crate::di::test_helpers::TestProviderModule;
///
/// let module = TestProviderModule::new()
///     .with_operation_provider_factory(|backend| {
///         Arc::new(SqlOperationProvider::new(backend, "block".into(), "block".into(), "block".into()))
///     });
///
/// services.add_module_mut(module);
/// ```
///
/// # Example with direct provider (for providers that don't need the backend)
/// ```rust
/// use crate::di::test_helpers::TestProviderModule;
///
/// let module = TestProviderModule::new()
///     .with_operation_provider(my_provider);
///
/// services.add_module_mut(module);
/// ```
pub struct TestProviderModule {
    operation_providers: std::sync::Mutex<Vec<Arc<dyn OperationProvider>>>,
    operation_provider_factories: std::sync::Mutex<Vec<OperationProviderFactory>>,
    syncable_providers: std::sync::Mutex<Vec<(String, Arc<dyn SyncableProvider>)>>,
    syncable_provider_factories: std::sync::Mutex<Vec<(String, SyncableProviderFactory)>>,
}

impl TestProviderModule {
    /// Create a new TestProviderModule
    pub fn new() -> Self {
        Self {
            operation_providers: std::sync::Mutex::new(Vec::new()),
            operation_provider_factories: std::sync::Mutex::new(Vec::new()),
            syncable_providers: std::sync::Mutex::new(Vec::new()),
            syncable_provider_factories: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Add an operation provider directly (for providers that don't need the backend)
    pub fn with_operation_provider(self, provider: Arc<dyn OperationProvider>) -> Self {
        self.operation_providers.lock().unwrap().push(provider);
        self
    }

    /// Add an operation provider factory that receives the backend
    ///
    /// This is the recommended way to add providers that need database access,
    /// as the factory will receive the correct backend instance.
    pub fn with_operation_provider_factory<F>(self, factory: F) -> Self
    where
        F: FnOnce(Arc<RwLock<TursoBackend>>) -> Arc<dyn OperationProvider> + Send + 'static,
    {
        self.operation_provider_factories
            .lock()
            .unwrap()
            .push(Box::new(factory));
        self
    }

    /// Add a syncable provider directly
    pub fn with_syncable_provider(self, name: String, provider: Arc<dyn SyncableProvider>) -> Self {
        self.syncable_providers
            .lock()
            .unwrap()
            .push((name, provider));
        self
    }

    /// Add a syncable provider factory that receives the backend
    pub fn with_syncable_provider_factory<F>(self, name: String, factory: F) -> Self
    where
        F: FnOnce(Arc<RwLock<TursoBackend>>) -> Arc<dyn SyncableProvider> + Send + 'static,
    {
        self.syncable_provider_factories
            .lock()
            .unwrap()
            .push((name, Box::new(factory)));
        self
    }
}

impl Default for TestProviderModule {
    fn default() -> Self {
        Self::new()
    }
}

impl Module for TestProviderModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        // Register direct operation providers as trait factories
        for provider in self.operation_providers.lock().unwrap().drain(..) {
            let provider_clone = provider.clone();
            injector.provide_into_set::<dyn OperationProvider>(Provider::root(move |_resolver| {
                provider_clone.clone()
            }));
        }

        // Register factory-based operation providers
        // These receive the backend at creation time, matching how production code works
        for factory in self.operation_provider_factories.lock().unwrap().drain(..) {
            // Wrap the factory in a Mutex so we can move it into the closure and call it once
            let factory_mutex = std::sync::Mutex::new(Some(factory));

            injector.provide_into_set::<dyn OperationProvider>(Provider::root(move |resolver| {
                // Get the backend via TursoBackendProvider trait (registered in register_core_services_with_backend)
                let provider = resolver.resolve::<dyn TursoBackendProvider>();
                let backend = provider.backend();
                let factory = factory_mutex
                    .lock()
                    .expect("Mutex poisoned")
                    .take()
                    .expect("Provider factory should only be called once");
                factory(backend)
            }));
        }

        // Register direct syncable providers as trait factories
        for (_name, provider) in self.syncable_providers.lock().unwrap().drain(..) {
            let provider_clone = provider.clone();
            injector.provide_into_set::<dyn SyncableProvider>(Provider::root(move |_resolver| {
                provider_clone.clone()
            }));
        }

        // Register factory-based syncable providers
        for (_name, factory) in self.syncable_provider_factories.lock().unwrap().drain(..) {
            let factory_mutex = std::sync::Mutex::new(Some(factory));

            injector.provide_into_set::<dyn SyncableProvider>(Provider::root(move |resolver| {
                let backend = resolver.resolve::<RwLock<TursoBackend>>();
                let factory = factory_mutex
                    .lock()
                    .expect("Mutex poisoned")
                    .take()
                    .expect("Syncable provider factory should only be called once");
                factory(backend)
            }));
        }

        // Note: OperationModule is already registered by register_core_services.
        // We don't need to register it again - the providers we just added will
        // be collected when OperationDispatcher is resolved.

        Ok(())
    }
}

/// Create a test engine with providers registered via TestProviderModule
///
/// This is a convenience function that makes it easy to create a test engine
/// with custom providers using the builder pattern.
///
/// # Arguments
/// * `db_path` - Path to the database file (use ":memory:" for in-memory)
/// * `setup_fn` - Closure that builds a TestProviderModule with providers
///
/// # Example
/// ```rust
/// let engine = create_test_engine_with_providers(":memory:".into(), |module| {
///     module
///         .with_operation_provider(my_provider)
///         .with_syncable_provider("todoist", todoist_provider)
/// }).await.unwrap();
/// ```
pub async fn create_test_engine_with_providers<F>(
    db_path: PathBuf,
    setup_fn: F,
) -> Result<Arc<BackendEngine>>
where
    F: FnOnce(TestProviderModule) -> TestProviderModule,
{
    create_backend_engine(db_path, |services| {
        let provider_module = setup_fn(TestProviderModule::new());
        provider_module
            .configure(services)
            .map_err(|e| anyhow::anyhow!("Failed to register TestProviderModule: {}", e))?;
        Ok(())
    })
    .await
}

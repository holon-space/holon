//! Dependency Injection module for Todoist integration
//!
//! This module provides DI registration for Todoist-specific services using fluxdi.
//!
//! # Usage
//!
//! Use the [`TodoistInjectorExt`] extension trait to register Todoist services:
//!
//! ```rust,ignore
//! use holon_todoist::di::TodoistInjectorExt;
//!
//! services.add_todoist(TodoistConfig::new(Some("your-api-key".into())))?;
//! ```

use std::sync::Arc;

use fluxdi::{Injector, Module, Provider, Shared};

use crate::TodoistClient;
use crate::TodoistSyncProvider;
use crate::api_client::TodoistApiClient;
#[cfg(not(target_arch = "wasm32"))]
use crate::fake_client::TodoistFakeClient;
use crate::models::{TodoistProject, TodoistTask};
use crate::todoist_datasource::{TodoistProjectDataSource, TodoistTaskOperations};
use crate::todoist_event_adapter::TodoistEventAdapter;
use holon::core::datasource::{OperationProvider, SyncTokenStore, SyncableProvider};
use holon::core::operation_wrapper::OperationWrapper;
use holon::core::queryable_cache::QueryableCache;
use holon::sync::{EventBus, TursoEventBus};
use holon::type_registry::TypeRegistry;
use holon_mcp_client::{EntityFieldReader, McpOperationProvider, McpSidecar};

/// Configuration for Todoist integration
///
/// Supports two modes:
/// - Real mode: Uses actual Todoist API with provided `api_key`
/// - Fake mode: Uses in-memory fake client for testing (enabled via `use_fake`)
pub const DEFAULT_TODOIST_MCP_URI: &str = "https://ai.todoist.net/mcp";

/// Configuration for Todoist integration
///
/// Supports two modes:
/// - Real mode: Uses actual Todoist API with provided `api_key`
/// - Fake mode: Uses in-memory fake client for testing (enabled via `use_fake`)
///
/// Deserializable from TOML (e.g. `holon.toml`):
/// ```toml
/// [todoist]
/// api_key = "your-key"
/// mcp_server_uri = "https://ai.todoist.net/mcp"  # default
/// ```
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
#[serde(default)]
pub struct TodoistConfig {
    pub api_key: Option<String>,
    #[serde(default)]
    pub use_fake: bool,
    /// HTTP request timeout in seconds (default: 30)
    #[serde(default = "default_timeout_secs")]
    pub timeout_secs: u64,
    /// Maximum number of retries for transient failures (default: 3)
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    /// URI for the Todoist MCP server. Set to enable MCP operations.
    /// Example: `mcp_server_uri = "https://ai.todoist.net/mcp"`
    pub mcp_server_uri: Option<String>,
}

fn default_timeout_secs() -> u64 {
    30
}
fn default_max_retries() -> u32 {
    3
}

impl Default for TodoistConfig {
    fn default() -> Self {
        Self {
            api_key: None,
            use_fake: false,
            timeout_secs: 30,
            max_retries: 3,
            mcp_server_uri: None,
        }
    }
}

impl TodoistConfig {
    pub fn new(api_key: Option<String>) -> Self {
        Self {
            api_key,
            ..Default::default()
        }
    }

    /// Create a config with MCP server URI for operation discovery.
    pub fn with_mcp(mut self, uri: String) -> Self {
        self.mcp_server_uri = Some(uri);
        self
    }

    /// Create a fake config for testing.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn fake() -> Self {
        Self {
            use_fake: true,
            mcp_server_uri: None,
            ..Default::default()
        }
    }
}

/// ServiceModule for Todoist integration
///
/// Registers Todoist-specific services in the DI container:
/// - `TodoistConfig` - Configuration with API key
/// - `Arc<dyn SyncableProvider>` - The syncable provider (if API key is provided)
/// - `QueryableCache` for tasks and projects (populated via change streams)
/// - `TodoistTaskOperations` - Operations using cache for lookups
///
/// Note: Providers are registered as `Arc<dyn SyncableProvider>` (not wrapped in Mutex)
/// because `sync()` no longer requires `&mut self` - it takes and returns `StreamPosition`.
pub struct TodoistModule {
    register_mcp: bool,
}

impl Module for TodoistModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        use std::println;
        use tracing::info;

        println!("[TodoistModule] register_services called");
        info!("[TodoistModule] register_services called");

        // Register TodoistSyncProvider as a factory that reads TodoistConfig and SyncTokenStore from DI
        // This allows the API key to be passed via DI instead of environment variables
        // Note: This factory will only be called if TodoistConfig is registered.
        // If TodoistConfig is not registered, don't register TodoistModule.
        injector.provide::<TodoistSyncProvider>(Provider::root(|resolver| {
            
            use std::println;
        
            println!("[TodoistModule] TodoistSyncProvider factory called");
        
            // Get TodoistConfig from DI (required - should be registered before TodoistModule)
            let config = match resolver.try_resolve::<TodoistConfig>() {
                Ok(c) => {
                    println!("[TodoistModule] TodoistConfig found in DI");
                    c
                }
                Err(e) => {
                    let msg = format!("[TodoistModule] ERROR: TodoistConfig not found in DI! Make sure TodoistConfig is registered before TodoistModule. Error: {}", e);
                    println!("{}", msg);
                    tracing::debug!("{}", msg);
                    panic!("{}", msg);
                }
            };
        
            // Get SyncTokenStore from DI (required - should be registered in core services)
            // When using add_trait_factory, use get_trait() instead of get() for trait objects
            // get_trait returns Arc<dyn Trait> directly (not wrapped in another Arc)
            // Use custom error handling to avoid panic message that FRB tries to decode
            let token_store = resolver
                            .try_resolve::<dyn SyncTokenStore>()
                .unwrap_or_else(|e| {
                    let msg = "[TodoistModule] ERROR: SyncTokenStore not found in DI! Make sure it's registered in core services.";
                    println!("{} Error: {:?}", msg, e);
                    tracing::debug!("{} Error: {:?}", msg, e);
                    panic!("{}", msg);
                });
        
            // Create the appropriate client based on config
            let client: Arc<dyn TodoistApiClient> = if config.use_fake {
                #[cfg(not(target_arch = "wasm32"))]
                {
                    println!("[TodoistModule] Using fake Todoist client (testing mode)");
                    info!("[TodoistModule] Using fake Todoist client (testing mode)");
                    Arc::new(TodoistFakeClient::new())
                }
                #[cfg(target_arch = "wasm32")]
                {
                    panic!("[TodoistModule] Fake client is not available on WASM target");
                }
            } else if let Some(api_key) = &config.api_key {
                println!("[TodoistModule] API key found in TodoistConfig, setting up Todoist integration");
                info!("[TodoistModule] API key found in TodoistConfig, setting up Todoist integration");
                Arc::new(TodoistClient::with_config(api_key, config.timeout_secs, config.max_retries))
            } else {
                // TodoistConfig registered but no API key and not fake mode - this is a configuration error
                let msg = "[TodoistModule] ERROR: TodoistConfig registered but no API key provided and not in fake mode. Either provide an API key, enable fake mode, or don't register TodoistModule.";
                println!("{}", msg);
                tracing::debug!("{}", msg);
                panic!("{}", msg);
            };
        
            Shared::new(TodoistSyncProvider::new(client, token_store))
        }));

        // Register SyncableProvider trait implementation (for sync operations)
        // This factory will only succeed if TodoistConfig has an API key
        injector.provide_into_set::<dyn SyncableProvider>(Provider::root(|resolver| {
            // fluxdi wraps in Arc, so we get Arc<TodoistSyncProvider>
            let sync_provider = resolver.resolve::<TodoistSyncProvider>();
            // Clone and cast to trait object
            sync_provider.clone() as Arc<dyn SyncableProvider>
        }));

        // Register OperationProvider trait implementation (for sync operation discovery)
        // TodoistSyncProvider implements OperationProvider to provide "todoist.sync" operation
        injector.provide_into_set::<dyn OperationProvider>(Provider::root(|resolver| {
            let sync_provider = resolver.resolve::<TodoistSyncProvider>();
            sync_provider.clone() as Arc<dyn OperationProvider>
        }));

        // Register Todoist entity types in the TypeRegistry for GQL graph,
        // then create their QueryableCaches.
        injector.provide::<QueryableCache<TodoistTask>>(Provider::root_async(|r| async move {
            let type_registry = r.resolve::<TypeRegistry>();
            if let Err(e) = type_registry.register(TodoistTask::type_definition()) {
                tracing::warn!("[TodoistModule] Failed to register TodoistTask type: {e}");
            }
            Shared::new(holon::di::create_queryable_cache_async(&r).await)
        }));
        injector.provide::<QueryableCache<TodoistProject>>(Provider::root_async(|r| async move {
            let type_registry = r.resolve::<TypeRegistry>();
            if let Err(e) = type_registry.register(TodoistProject::type_definition()) {
                tracing::warn!("[TodoistModule] Failed to register TodoistProject type: {e}");
            }
            Shared::new(holon::di::create_queryable_cache_async(&r).await)
        }));

        // Register TodoistTaskOperations (uses cache for lookups, provider for API mutations)
        injector.provide::<TodoistTaskOperations>(Provider::root_async(|resolver| async move {
            println!("[TodoistModule] TodoistTaskOperations factory called");

            let cache = resolver
                .resolve_async::<QueryableCache<TodoistTask>>()
                .await;
            let sync_provider = resolver.resolve::<TodoistSyncProvider>();

            println!("[TodoistModule] TodoistTaskOperations created");
            Shared::new(TodoistTaskOperations::new(cache, sync_provider))
        }));

        // Register TodoistTaskOperations as OperationProvider and set up stream subscriptions
        // This enables operations like set_field to work on todoist_tasks
        //
        // IMPORTANT: This factory is called during BackendEngine creation, which happens in the
        // launcher's async context on the main runtime. This means we can safely subscribe the
        // cache to the stream here - tokio::spawn will use the main runtime, not a temporary one.
        injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(|resolver| async move {
            use tracing::info;

            // Get caches (async — DDL runs in factory)
            let task_cache = resolver.resolve_async::<QueryableCache<TodoistTask>>().await;
            let project_cache = resolver.resolve_async::<QueryableCache<TodoistProject>>().await;

            // Get operations handler
            let task_ops = resolver.resolve_async::<TodoistTaskOperations>().await;

            // Get sync provider to subscribe to its streams and for post-operation sync
            let sync_provider = resolver.resolve::<TodoistSyncProvider>();

            // Subscribe task cache to sync provider's task stream with metadata
            info!("[Todoist] Subscribing task cache to sync provider stream with metadata");
            let task_rx = sync_provider.subscribe_tasks();
            task_cache.ingest_stream_with_metadata(task_rx);
            info!("[Todoist] Task stream subscription complete!");

            // Subscribe project cache to sync provider's project stream with metadata
            info!("[Todoist] Subscribing project cache to sync provider stream with metadata");
            let project_rx = sync_provider.subscribe_projects();
            project_cache.ingest_stream_with_metadata(project_rx);
            info!("[Todoist] Project stream subscription complete!");

            // Wire TodoistSyncProvider → EventBus (via TodoistEventAdapter)
            {
                if let Ok(event_bus) = resolver.try_resolve_async::<TursoEventBus>().await {
                    let sync_provider_clone = sync_provider.clone();
                    let event_bus_arc: Arc<dyn EventBus> = event_bus.clone();
                    tokio::spawn(async move {
                        let adapter = TodoistEventAdapter::new(event_bus_arc);
                        let task_rx = sync_provider_clone.subscribe_tasks();
                        let project_rx = sync_provider_clone.subscribe_projects();
                        if let Err(e) = adapter.start(task_rx, project_rx) {
                            tracing::error!("[Todoist] Failed to start TodoistEventAdapter: {}", e);
                        } else {
                            info!("[Todoist] TodoistEventAdapter started: Todoist → EventBus");
                        }
                    });
                } else {
                    info!("[Todoist] EventBus not found in DI, skipping TodoistEventAdapter wiring");
                }
            }

            // Wrap TodoistTaskOperations with OperationWrapper for automatic post-operation sync
            let wrapped = OperationWrapper::new(task_ops, Some(sync_provider.clone()));
            info!("[Todoist] TodoistTaskOperations wrapped with OperationWrapper for auto-sync");

            Arc::new(wrapped) as Arc<dyn OperationProvider>
        }));

        // Register TodoistProjectDataSource as a separate OperationProvider
        // This enables move_block operations on todoist_projects
        // We use the datasource directly (not the cache) since TodoistProject
        // doesn't implement OperationRegistry (projects don't have the same
        // complex operations that tasks do)
        injector.provide::<TodoistProjectDataSource>(Provider::root(|resolver| {
            let sync_provider = resolver.resolve::<TodoistSyncProvider>();
            Shared::new(TodoistProjectDataSource::new(sync_provider.clone()))
        }));
        injector.provide_into_set::<dyn OperationProvider>(Provider::root(|resolver| {
            use tracing::info;

            let project_ops = resolver.resolve::<TodoistProjectDataSource>();
            let sync_provider = resolver.resolve::<TodoistSyncProvider>();

            // Wrap TodoistProjectDataSource with OperationWrapper for automatic post-operation sync
            let wrapped = OperationWrapper::new(project_ops, Some(sync_provider.clone()));
            info!("[Todoist] TodoistProjectDataSource wrapped with OperationWrapper for auto-sync");

            Arc::new(wrapped) as Arc<dyn OperationProvider>
        }));

        // Only register McpOperationProvider when an MCP server URI is configured.
        // McpOperationProvider::connect() is async; block_on is safe here because
        // DI factories run from the launcher's async context on the main tokio runtime.
        if self.register_mcp {
            injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
                |resolver| async move {
                    use holon::core::datasource::OperationProvider;
                    use tracing::info;

                    let config = resolver.resolve::<TodoistConfig>();
                    let mcp_uri = config
                        .mcp_server_uri
                        .as_deref()
                        .expect("[TodoistModule] register_mcp=true but mcp_server_uri is None");

                    let sidecar_yaml = include_str!("../todoist_mcp_operations.yaml");
                    let sidecar = McpSidecar::from_yaml(sidecar_yaml)
                        .expect("[TodoistModule] Failed to parse todoist_mcp_operations.yaml");

                    let task_cache = resolver
                        .resolve_async::<QueryableCache<TodoistTask>>()
                        .await;
                    let project_cache = resolver
                        .resolve_async::<QueryableCache<TodoistProject>>()
                        .await;
                    let mut entity_readers: std::collections::HashMap<
                        String,
                        Arc<dyn EntityFieldReader>,
                    > = std::collections::HashMap::new();
                    entity_readers.insert(
                        "todoist_task".to_string(),
                        task_cache as Arc<dyn EntityFieldReader>,
                    );
                    entity_readers.insert(
                        "todoist_project".to_string(),
                        project_cache as Arc<dyn EntityFieldReader>,
                    );

                    let auth_token = config.api_key.as_deref();
                    info!("[Todoist] Connecting to MCP server at {}", mcp_uri);
                    let provider = McpOperationProvider::connect(
                        mcp_uri,
                        auth_token,
                        sidecar,
                        entity_readers,
                    )
                    .await
                    .expect("[TodoistModule] Failed to connect MCP / build McpOperationProvider");
                    info!(
                        "[Todoist] McpOperationProvider ready with {} operations",
                        provider.operations().len()
                    );

                    let sync_provider = resolver.resolve::<TodoistSyncProvider>();
                    let wrapped =
                        OperationWrapper::new(Arc::new(provider), Some(sync_provider.clone()));
                    info!(
                        "[Todoist] McpOperationProvider wrapped with OperationWrapper for auto-sync"
                    );

                    Arc::new(wrapped) as Arc<dyn OperationProvider>
                },
            ));
        }

        Ok(())
    }
}

/// Extension trait for registering Todoist services in a [`Injector`]
///
/// # Example
///
/// ```rust,ignore
/// use holon_todoist::di::{TodoistInjectorExt, TodoistConfig};
///
/// services.add_todoist(TodoistConfig::new(Some("api-key".into())))?;
/// ```
pub trait TodoistInjectorExt {
    fn add_todoist(&self, config: TodoistConfig) -> std::result::Result<(), fluxdi::Error>;

    #[cfg(not(target_arch = "wasm32"))]
    fn add_todoist_fake(&self) -> std::result::Result<(), fluxdi::Error>;
}

impl TodoistInjectorExt for Injector {
    fn add_todoist(&self, config: TodoistConfig) -> std::result::Result<(), fluxdi::Error> {
        let register_mcp = config.mcp_server_uri.is_some();
        self.provide::<TodoistConfig>(Provider::root(move |_| Shared::new(config.clone())));
        TodoistModule { register_mcp }.configure(self)?;
        Ok(())
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn add_todoist_fake(&self) -> std::result::Result<(), fluxdi::Error> {
        self.provide::<TodoistConfig>(Provider::root(move |_| Shared::new(TodoistConfig::fake())));
        TodoistModule {
            register_mcp: false,
        }
        .configure(self)?;
        Ok(())
    }
}

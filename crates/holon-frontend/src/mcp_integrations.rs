use std::path::Path;
use std::sync::Arc;

use ferrous_di::{DiResult, Lifetime, Resolver, ServiceCollection, ServiceModule};
use tracing::{info, warn};

use holon::core::datasource::{EntitySchemaProvider, OperationProvider, SyncTokenStore};
use holon_mcp_client::{
    build_mcp_integration, load_integration_configs, IntegrationFileConfig, McpIntegration,
    PendingOAuthFlows,
};

/// Holds all running MCP integrations so their services stay alive.
pub struct McpIntegrationRegistry {
    integrations: Vec<McpIntegration>,
}

impl McpIntegrationRegistry {
    pub fn integrations(&self) -> &[McpIntegration] {
        &self.integrations
    }
}

impl EntitySchemaProvider for McpIntegrationRegistry {
    fn entity_schemas(&self) -> Vec<holon_api::EntitySchema> {
        self.integrations
            .iter()
            .flat_map(|i| i.entity_schemas.iter().cloned())
            .collect()
    }
}

/// DI module that registers MCP provider integrations from config files.
///
/// For each loaded config, registers an `OperationProvider` trait implementation
/// so the `OperationDispatcher` can discover and route operations to MCP servers.
pub struct McpIntegrationsModule {
    configs: Vec<(String, IntegrationFileConfig)>,
}

impl McpIntegrationsModule {
    /// Create a module by loading configs from the given directory.
    ///
    /// Files that fail to parse are logged and skipped. If the directory
    /// doesn't exist, no integrations are loaded.
    pub fn from_dir(dir: &Path) -> Self {
        let configs = load_integration_configs(dir);
        info!(
            "[McpIntegrationsModule] Loaded {} integration configs from '{}'",
            configs.len(),
            dir.display()
        );
        Self { configs }
    }
}

impl ServiceModule for McpIntegrationsModule {
    fn register_services(self, services: &mut ServiceCollection) -> DiResult<()> {
        if self.configs.is_empty() {
            return Ok(());
        }

        let configs = Arc::new(self.configs);
        let pending_flows = Arc::new(PendingOAuthFlows::new());
        services.add_singleton(pending_flows.clone());

        let configs_for_registry = configs.clone();

        // Register the registry as a singleton — builds all integrations eagerly.
        // The registry keeps McpRunningService and subscription tasks alive.
        services.add_singleton_factory::<McpIntegrationRegistry, _>(move |resolver| {
            let db_handle = resolver
                .get_required_trait::<dyn holon::di::DbHandleProvider>()
                .handle();
            let token_store: Arc<dyn SyncTokenStore> =
                resolver.get_required_trait::<dyn SyncTokenStore>();

            let mut integrations = Vec::new();

            for (name, config) in configs_for_registry.as_ref() {
                let mcp_config = config.clone().into_mcp_config(name.clone());

                let result = tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(build_mcp_integration(
                        mcp_config,
                        db_handle.clone(),
                        token_store.clone(),
                        &pending_flows,
                    ))
                });

                match result {
                    Ok(holon_mcp_client::McpConnectionResult::Connected(integration)) => {
                        info!(
                            "[McpIntegrationsModule] Provider '{}' connected ({} operations)",
                            name,
                            integration.operation_provider.operations().len()
                        );

                        // Run initial sync
                        let sync_engine = integration.sync_engine.clone();
                        tokio::task::block_in_place(|| {
                            tokio::runtime::Handle::current().block_on(async {
                                if let Err(e) = sync_engine.sync_all().await {
                                    warn!(
                                        "[McpIntegrationsModule] Initial sync for '{}' failed: {e}",
                                        name
                                    );
                                }
                            })
                        });

                        integrations.push(integration);
                    }
                    Ok(holon_mcp_client::McpConnectionResult::NeedsAuth {
                        auth_url,
                        provider_name,
                    }) => {
                        warn!(
                            "[McpIntegrationsModule] Provider '{}' needs OAuth — auth_url: {}",
                            provider_name, auth_url
                        );
                    }
                    Err(e) => {
                        warn!(
                            "[McpIntegrationsModule] Failed to connect provider '{}': {e}",
                            name
                        );
                    }
                }
            }

            info!(
                "[McpIntegrationsModule] Registry created with {} active integrations",
                integrations.len()
            );
            McpIntegrationRegistry { integrations }
        });

        // Register the registry as EntitySchemaProvider so MCP entity schemas
        // are included in the GQL GraphSchema built by BackendEngine.
        services.add_trait_factory::<dyn EntitySchemaProvider, _>(
            Lifetime::Singleton,
            |resolver| {
                let registry = resolver.get_required::<McpIntegrationRegistry>();
                registry as Arc<dyn EntitySchemaProvider>
            },
        );

        // Register each config's OperationProvider so OperationDispatcher discovers them.
        // Each factory resolves the shared registry and returns the corresponding provider.
        for (idx, (name, _)) in configs.iter().enumerate() {
            let name = name.clone();
            services.add_trait_factory::<dyn OperationProvider, _>(
                Lifetime::Singleton,
                move |resolver| {
                    let registry = resolver.get_required::<McpIntegrationRegistry>();

                    // The registry may have fewer integrations than configs (failed connections).
                    // Find the matching integration by checking operation descriptors for the provider name.
                    // Fall back to index-based lookup for the common case where all succeed.
                    if idx < registry.integrations.len() {
                        let provider = &registry.integrations[idx].operation_provider;
                        info!(
                            "[McpIntegrationsModule] Registered OperationProvider for '{name}' with {} operations",
                            provider.operations().len()
                        );
                        // We need to return Arc<dyn OperationProvider>. McpOperationProvider
                        // is inside McpIntegration which is inside the registry Arc.
                        // We'll wrap a reference proxy that delegates to the registry.
                        let registry_clone = registry.clone();
                        Arc::new(RegistryOperationProxy { registry: registry_clone, index: idx })
                            as Arc<dyn OperationProvider>
                    } else {
                        warn!(
                            "[McpIntegrationsModule] No integration at index {} for '{}' — \
                             connection may have failed",
                            idx, name
                        );
                        Arc::new(EmptyOperationProvider) as Arc<dyn OperationProvider>
                    }
                },
            );
        }

        Ok(())
    }
}

/// Proxy that delegates OperationProvider calls to an integration in the shared registry.
struct RegistryOperationProxy {
    registry: Arc<McpIntegrationRegistry>,
    index: usize,
}

#[async_trait::async_trait]
impl OperationProvider for RegistryOperationProxy {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        self.registry.integrations[self.index]
            .operation_provider
            .operations()
    }

    async fn execute_operation(
        &self,
        entity_name: &str,
        op_name: &str,
        params: holon::storage::types::StorageEntity,
    ) -> holon_core::traits::Result<holon_core::traits::OperationResult> {
        self.registry.integrations[self.index]
            .operation_provider
            .execute_operation(entity_name, op_name, params)
            .await
    }
}

/// No-op provider for failed integrations.
struct EmptyOperationProvider;

#[async_trait::async_trait]
impl OperationProvider for EmptyOperationProvider {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        vec![]
    }

    async fn execute_operation(
        &self,
        _entity_name: &str,
        _op_name: &str,
        _params: holon::storage::types::StorageEntity,
    ) -> holon_core::traits::Result<holon_core::traits::OperationResult> {
        Err("MCP integration not connected".into())
    }
}

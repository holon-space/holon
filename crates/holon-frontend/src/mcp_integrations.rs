use std::path::Path;
use std::sync::Arc;

use fluxdi::{Injector, Module, Provider, Shared};
use tracing::{info, warn};

use holon::core::datasource::{OperationProvider, SyncTokenStore};
use holon::type_registry::TypeRegistry;
use holon_api::EntityName;
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

    /// All cache table names that have FDW backing, across all integrations.
    pub fn fdw_backed_tables(&self) -> Vec<String> {
        self.integrations
            .iter()
            .flat_map(|i| i.fdw_backed_tables.iter().cloned())
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

impl Module for McpIntegrationsModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        if self.configs.is_empty() {
            return Ok(());
        }

        let configs = Arc::new(self.configs.clone());
        let pending_flows = Arc::new(PendingOAuthFlows::new());
        let pending_flows_clone = pending_flows.clone();
        injector.provide::<PendingOAuthFlows>(Provider::root(move |_| pending_flows_clone.clone()));

        let configs_for_registry = configs.clone();

        // Register the registry as an async singleton — resolved in parallel with other DI services.
        injector.provide::<McpIntegrationRegistry>(Provider::root_async(move |resolver| {
            let configs_for_registry = configs_for_registry.clone();
            let pending_flows = pending_flows.clone();
            async move {
                let db_handle = resolver
                    .resolve_async::<dyn holon::di::DbHandleProvider>()
                    .await
                    .handle();
                let token_store: Arc<dyn SyncTokenStore> =
                    resolver.resolve_async::<dyn SyncTokenStore>().await;
                let type_registry = resolver.resolve::<TypeRegistry>();

                let mut integrations = Vec::new();

                for (name, config) in configs_for_registry.as_ref() {
                    let mcp_config = config.clone().into_mcp_config(name.clone());

                    let result = build_mcp_integration(
                        mcp_config,
                        db_handle.clone(),
                        token_store.clone(),
                        &pending_flows,
                    )
                    .await;

                    match result {
                        Ok(holon_mcp_client::McpConnectionResult::Connected(integration)) => {
                            info!(
                                "[McpIntegrationsModule] Provider '{}' connected ({} operations)",
                                name,
                                integration.operation_provider.operations().len()
                            );

                            // Register MCP entity types in TypeRegistry for GQL graph
                            integration.register_entity_types(&type_registry);

                            // Spawn initial sync in background — don't block startup
                            let sync_engine = integration.sync_engine.clone();
                            let sync_name = name.clone();
                            tokio::spawn(async move {
                                if let Err(e) = sync_engine.sync_all().await {
                                    warn!(
                                        "[McpIntegrationsModule] Initial sync for '{}' failed: {e}",
                                        sync_name
                                    );
                                }
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
                Shared::new(McpIntegrationRegistry { integrations })
            }
        }));

        // Register each config's OperationProvider so OperationDispatcher discovers them.
        // Each factory resolves the shared registry and returns the corresponding provider.
        for (idx, (name, _)) in configs.iter().enumerate() {
            let name = name.clone();
            injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
                move |resolver| {
                    let name = name.clone();
                    async move {
                        let registry =
                            resolver.resolve_async::<McpIntegrationRegistry>().await;

                        // The registry may have fewer integrations than configs (failed connections).
                        if idx < registry.integrations.len() {
                            let provider = &registry.integrations[idx].operation_provider;
                            info!(
                                "[McpIntegrationsModule] Registered OperationProvider for '{name}' with {} operations",
                                provider.operations().len()
                            );
                            let registry_clone = registry.clone();
                            Arc::new(RegistryOperationProxy {
                                registry: registry_clone,
                                index: idx,
                            }) as Arc<dyn OperationProvider>
                        } else {
                            warn!(
                                "[McpIntegrationsModule] No integration at index {} for '{}' — \
                                 connection may have failed",
                                idx, name
                            );
                            Arc::new(EmptyOperationProvider) as Arc<dyn OperationProvider>
                        }
                    }
                },
            ));
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
        entity_name: &EntityName,
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
        _entity_name: &EntityName,
        _op_name: &str,
        _params: holon::storage::types::StorageEntity,
    ) -> holon_core::traits::Result<holon_core::traits::OperationResult> {
        Err("MCP integration not connected".into())
    }
}

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use fluxdi::{Injector, Module, Provider, Shared};
use tracing::{info, warn};

use holon::core::datasource::{OperationProvider, SyncTokenStore};
use holon::type_registry::TypeRegistry;
use holon_api::EntityName;
use holon_mcp_client::integration_config::UnresolvedVar;
use holon_mcp_client::{
    build_mcp_integration, load_integration_configs, IntegrationFileConfig, McpIntegration,
    PendingOAuthFlows,
};

/// Normalize a variable/setting name for fuzzy matching: lowercase, with `.`
/// and `_` treated as the same separator. So the env var `TODOIST_API_KEY` and
/// the setting key `todoist.api_key` both normalize to `todoist_api_key`.
fn normalize_var_name(s: &str) -> String {
    s.to_ascii_lowercase().replace('.', "_")
}

/// Holds all running MCP integrations so their services stay alive.
///
/// Integrations are keyed by provider name (`names[i]` belongs to
/// `integrations[i]`) so lookups never rely on positional alignment with the
/// original config list — skipped or failed integrations leave no hole.
pub struct McpIntegrationRegistry {
    /// Provider names, parallel to `integrations`.
    names: Vec<String>,
    integrations: Vec<McpIntegration>,
}

impl McpIntegrationRegistry {
    pub fn integrations(&self) -> &[McpIntegration] {
        &self.integrations
    }

    /// Look up a connected integration by provider name.
    pub fn by_name(&self, name: &str) -> Option<&McpIntegration> {
        debug_assert_eq!(self.names.len(), self.integrations.len());
        self.names
            .iter()
            .position(|n| n == name)
            .map(|i| &self.integrations[i])
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
    /// Loaded configs, or the enriched load error (e.g. malformed YAML).
    /// The error is surfaced in `configure()` so it propagates through the
    /// module-registration `Result` instead of being swallowed here.
    configs: Result<Vec<(String, IntegrationFileConfig)>, String>,
}

impl McpIntegrationsModule {
    /// Create a module by loading configs from the given directory.
    ///
    /// A missing directory means no integrations. A YAML file that fails to
    /// read or parse is a hard error — it is captured here and returned from
    /// `configure()` (fail loud, never skip a malformed integration config).
    pub fn from_dir(dir: &Path) -> Self {
        let configs = load_integration_configs(dir).map_err(|e| format!("{e:#}"));
        if let Ok(configs) = &configs {
            info!(
                "[McpIntegrationsModule] Loaded {} integration configs from '{}'",
                configs.len(),
                dir.display()
            );
        }
        Self { configs }
    }
}

impl Module for McpIntegrationsModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        let configs = self.configs.as_ref().map_err(|msg| {
            fluxdi::Error::module_lifecycle_failed("McpIntegrationsModule", "configure", msg)
        })?;
        if configs.is_empty() {
            return Ok(());
        }

        let configs = Arc::new(configs.clone());
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

                // Layered `${VAR}` resolver: environment variable wins, then a
                // settings value whose key matches case-insensitively with `.`/`_`
                // treated as the same separator (so `${TODOIST_API_KEY}` resolves
                // from the `todoist.api_key` setting). Keeps secrets out of the
                // committed YAML while letting the Settings UI supply them.
                let pref_by_norm: HashMap<String, String> = resolver
                    .resolve::<holon_frontend::config::HolonConfig>()
                    .preferences
                    .iter()
                    .filter_map(|(k, v)| {
                        let s = v.as_str()?;
                        (!s.is_empty()).then(|| (normalize_var_name(k.as_str()), s.to_string()))
                    })
                    .collect();
                let var_lookup = move |name: &str| -> Option<String> {
                    std::env::var(name)
                        .ok()
                        .filter(|v| !v.is_empty())
                        .or_else(|| pref_by_norm.get(&normalize_var_name(name)).cloned())
                };

                let mut names = Vec::new();
                let mut integrations = Vec::new();

                for (name, config) in configs_for_registry.as_ref() {
                    let mcp_config = match config
                        .clone()
                        .into_mcp_config_with(name.clone(), &var_lookup)
                    {
                        Ok(c) => c,
                        // Disclosed skip: the config references a `${VAR}` that is
                        // set neither in the environment nor in settings — the
                        // integration is simply not configured yet (e.g. missing
                        // API key). Everything else is an invalid config and must
                        // fail loud; the DI factory has no Result channel, so
                        // panic with full context rather than silently skipping.
                        Err(e) if e.downcast_ref::<UnresolvedVar>().is_some() => {
                            warn!(
                                "[McpIntegrationsModule] Provider '{}' is not configured — \
                                 skipping: {e}",
                                name
                            );
                            continue;
                        }
                        Err(e) => {
                            panic!(
                                "[McpIntegrationsModule] Invalid integration config for \
                                 provider '{name}': {e:#}"
                            );
                        }
                    };

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

                            names.push(name.clone());
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
                Shared::new(McpIntegrationRegistry {
                    names,
                    integrations,
                })
            }
        }));

        // Register each config's OperationProvider so OperationDispatcher discovers them.
        // Each factory resolves the shared registry and looks its integration up BY NAME —
        // the registry may hold fewer integrations than configs (skipped/failed
        // connections), so positional indexing would misroute operations.
        for (name, _) in configs.iter() {
            let name = name.clone();
            injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
                move |resolver| {
                    let name = name.clone();
                    async move {
                        let registry =
                            resolver.resolve_async::<McpIntegrationRegistry>().await;

                        if let Some(integration) = registry.by_name(&name) {
                            info!(
                                "[McpIntegrationsModule] Registered OperationProvider for '{name}' with {} operations",
                                integration.operation_provider.operations().len()
                            );
                            let registry_clone = registry.clone();
                            Arc::new(RegistryOperationProxy {
                                registry: registry_clone,
                                name,
                            }) as Arc<dyn OperationProvider>
                        } else {
                            warn!(
                                "[McpIntegrationsModule] Integration '{name}' unavailable \
                                 (not configured or failed to connect) — registering inert provider"
                            );
                            Arc::new(EmptyOperationProvider { name })
                                as Arc<dyn OperationProvider>
                        }
                    }
                },
            ));
        }

        Ok(())
    }
}

/// Proxy that delegates OperationProvider calls to an integration in the
/// shared registry, looked up by provider name (never by position).
struct RegistryOperationProxy {
    registry: Arc<McpIntegrationRegistry>,
    name: String,
}

impl RegistryOperationProxy {
    fn integration(&self) -> &McpIntegration {
        // The proxy is only constructed after a successful by_name lookup and
        // the registry is immutable, so a miss is an impossible state.
        self.registry.by_name(&self.name).unwrap_or_else(|| {
            panic!(
                "MCP integration '{}' vanished from the registry — \
                 proxy/registry invariant violated",
                self.name
            )
        })
    }
}

#[async_trait::async_trait]
impl OperationProvider for RegistryOperationProxy {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        self.integration().operation_provider.operations()
    }

    async fn execute_operation(
        &self,
        entity_name: &EntityName,
        op_name: &str,
        params: holon::storage::types::StorageEntity,
    ) -> holon_core::traits::Result<holon_core::traits::OperationResult> {
        self.integration()
            .operation_provider
            .execute_operation(entity_name, op_name, params)
            .await
    }
}

/// Inert provider registered when an integration is unavailable (not
/// configured, OAuth pending, or failed to connect). Executing an operation
/// names the integration so the failure is attributable, never misrouted.
struct EmptyOperationProvider {
    name: String,
}

#[async_trait::async_trait]
impl OperationProvider for EmptyOperationProvider {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        vec![]
    }

    async fn execute_operation(
        &self,
        _: &EntityName,
        _: &str,
        _: holon::storage::types::StorageEntity,
    ) -> holon_core::traits::Result<holon_core::traits::OperationResult> {
        Err(format!(
            "MCP integration '{}' unavailable — not configured or failed to connect at startup",
            self.name
        )
        .into())
    }
}

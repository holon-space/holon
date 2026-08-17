use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Module;
use fluxdi::Provider;
use fluxdi::Shared;
use holon::sync::DegradedSignalBus;
use holon::sync::ShareDegraded;
use holon::sync::ShareDegradedReason;
use holon_api::EntityName;
use holon_core::OperationProvider;
use holon_core::SyncGate;
use holon_core::SyncTokenStore;
use holon_mcp_client::IgnoredReason;
use holon_mcp_client::IgnoredSidecar;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::LoadedIntegrations;
use holon_mcp_client::McpIntegration;
use holon_mcp_client::PendingOAuthFlows;
use holon_mcp_client::PendingWriteStore;
use holon_mcp_client::SupersededSidecar;
use holon_mcp_client::build_mcp_integration;
use holon_mcp_client::integration_config::UnresolvedVar;
use holon_mcp_client::load_integration_configs;
use holon_profiles::TypeRegistry;
use tracing::info;
use tracing::warn;

use crate::integrations_settings::IntegrationsSettingsVm;

/// Normalize a variable/setting name for fuzzy matching: lowercase, with `.`
/// and `_` treated as the same separator. So the env var `TODOIST_API_KEY` and
/// the setting key `todoist.api_key` both normalize to `todoist_api_key`.
fn normalize_var_name(s: &str) -> String {
    s.to_ascii_lowercase().replace('.', "_")
}

/// Disclose that `name` could not be connected at boot. Without this the
/// failure is log-only and every page backed by the integration's `cc_*`
/// tables renders blank as if the remote had no data.
fn disclose_connect_failure(name: &str, error: &anyhow::Error, bus: &DegradedSignalBus) {
    bus.emit(ShareDegraded {
        shared_tree_id: name.to_string(),
        reason: ShareDegradedReason::IntegrationConnectFailed {
            integration: name.to_string(),
            error: format!("{error:#}"),
        },
    });
}

/// Disclose that `name` is connectable but waiting on an OAuth grant — same
/// blank-page consequence as a failed connect, different remedy.
fn disclose_needs_auth(name: &str, auth_url: &str, bus: &DegradedSignalBus) {
    bus.emit(ShareDegraded {
        shared_tree_id: name.to_string(),
        reason: ShareDegradedReason::IntegrationNeedsAuth {
            integration: name.to_string(),
            auth_url: auth_url.to_string(),
        },
    });
}

/// Disclose that an installed sidecar was ignored in favour of the bundled
/// one. The integration works, so nothing else in the boot path would ever say
/// that the file on disk is not what is running.
fn disclose_superseded_sidecar(s: &SupersededSidecar, bus: &DegradedSignalBus) {
    bus.emit(ShareDegraded {
        shared_tree_id: s.provider.clone(),
        reason: ShareDegradedReason::IntegrationSidecarSuperseded {
            integration: s.provider.clone(),
            installed_path: s.installed_path.display().to_string(),
            bundled_source: s.bundled_source.to_string(),
            incompatibility: s.incompatibility.clone(),
        },
    });
}

/// Disclose that an installed sidecar produced no provider at all. Its pages
/// render blank exactly like a failed connect, but nothing else in the boot
/// path would say why — the file is present, so from the user's side it looks
/// like the integration should be running.
fn disclose_ignored_sidecar(s: &IgnoredSidecar, bus: &DegradedSignalBus) {
    let reason = match &s.reason {
        IgnoredReason::NotEnabled {
            state_path, remedy, ..
        } => ShareDegradedReason::IntegrationNotEnabled {
            integration: s.provider.clone(),
            installed_path: s.installed_path.display().to_string(),
            state_path: state_path.display().to_string(),
            remedy: remedy.clone(),
        },
        IgnoredReason::NotBundled => ShareDegradedReason::IntegrationSidecarNotBundled {
            provider: s.provider.clone(),
            installed_path: s.installed_path.display().to_string(),
        },
    };
    bus.emit(ShareDegraded {
        shared_tree_id: s.provider.clone(),
        reason,
    });
}

/// The boot log line for an installed sidecar that enabled nothing. The bus
/// carries the paths; the remedy is spelled out here, where a multi-line state
/// file fits.
fn log_ignored_sidecar(s: &IgnoredSidecar) {
    match &s.reason {
        IgnoredReason::NotEnabled {
            state_path,
            remedy,
            enabling_state_file,
        } => warn!(
            "[McpIntegrationsModule] Provider '{}' is NOT enabled, so '{}' does nothing — a \
             sidecar file is no longer the switch. To switch it on, run `{remedy}`, or write \
             '{}' yourself — ALL of it, a partial file is rejected:\n{}",
            s.provider,
            s.installed_path.display(),
            state_path.display(),
            enabling_state_file
        ),
        IgnoredReason::NotBundled => warn!(
            "[McpIntegrationsModule] '{}' names provider '{}', which this build does not ship — \
             it does nothing. Integrations are compiled in (crates/holon-mcp-client/src/\
             bundled_sidecars.rs); add it there and rebuild, or delete the file.",
            s.installed_path.display(),
            s.provider
        ),
    }
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
/// For each loaded config, registers an `OperationProvider` trait
/// implementation so the `OperationDispatcher` can discover and route
/// operations to MCP servers.
pub struct McpIntegrationsModule {
    /// The enablement authority plus the scan of the integrations directory, or
    /// the enriched load error (e.g. malformed YAML for a provider this build
    /// does not ship). The error is surfaced in `configure()` so it propagates
    /// through the module-registration `Result` instead of being swallowed
    /// here.
    loaded: Result<(Arc<IntegrationConfigStore>, LoadedIntegrations), String>,
}

impl McpIntegrationsModule {
    /// Create a module for the integrations in `dir`.
    ///
    /// Which ones run is the state store's call — `dir` supplies the store's
    /// files and any content overrides, never the enablement. A directory that
    /// cannot be read, or that holds two files for one provider, is a hard
    /// error: it is captured here and returned from `configure()` (fail loud,
    /// never boot on a half-read integrations directory).
    pub fn from_dir(dir: &Path) -> Self {
        let loaded = IntegrationConfigStore::load(dir)
            .map(Arc::new)
            .and_then(|store| load_integration_configs(dir, &store).map(|l| (store, l)))
            .map_err(|e| format!("{e:#}"));
        if let Ok((_, loaded)) = &loaded {
            // Logged here, not at disclosure time: the registry singleton is
            // resolved lazily, so the bus signal may never fire in a container
            // that never touches an integration — the log must not depend on it.
            for s in &loaded.superseded {
                warn!(
                    "[McpIntegrationsModule] Installed sidecar '{}' for provider '{}' was NOT \
                     used: {}. Running the sidecar bundled with this build ('{}') instead. To \
                     silence this, delete the installed file, or re-author it against this \
                     build's schema_version.",
                    s.installed_path.display(),
                    s.provider,
                    s.incompatibility,
                    s.bundled_source
                );
            }
            for s in &loaded.ignored {
                log_ignored_sidecar(s);
            }
            info!(
                "[McpIntegrationsModule] {} integration(s) enabled from '{}' ({} installed \
                 sidecar(s) superseded by the bundled copy, {} enabling nothing)",
                loaded.configs.len(),
                dir.display(),
                loaded.superseded.len(),
                loaded.ignored.len()
            );
        }
        Self { loaded }
    }
}

impl Module for McpIntegrationsModule {
    fn configure(&self, injector: &Injector) -> std::result::Result<(), fluxdi::Error> {
        let (store, loaded) = self.loaded.as_ref().map_err(|msg| {
            fluxdi::Error::module_lifecycle_failed("McpIntegrationsModule", "configure", msg)
        })?;

        // The enablement authority and the settings list it backs are
        // registered BEFORE the nothing-to-run early return below: a vault with
        // every integration switched off produces no config and no ignored
        // sidecar, and that is exactly the container in which the settings
        // surface is the user's only way to switch one on.
        let store_di = store.clone();
        injector.provide::<IntegrationConfigStore>(Provider::root(move |_| store_di.clone()));
        let settings_vm = Arc::new(IntegrationsSettingsVm::new(store.clone()));
        injector.provide::<IntegrationsSettingsVm>(Provider::root(move |_| settings_vm.clone()));

        let configs = &loaded.configs;
        let superseded = Arc::new(loaded.superseded.clone());
        let ignored = Arc::new(loaded.ignored.clone());
        // Nothing to run AND nothing to say: leave the container untouched, so a
        // build with no integrations directory keeps resolving no MCP services
        // at all. Files that enabled nothing are the opposite case — the
        // registry factory is where the disclosure reaches the bus, so it must
        // be registered even when no integration runs.
        if configs.is_empty() && ignored.is_empty() {
            return Ok(());
        }

        let configs = Arc::new(configs.clone());
        let pending_flows = Arc::new(PendingOAuthFlows::new());
        let pending_flows_clone = pending_flows.clone();
        injector.provide::<PendingOAuthFlows>(Provider::root(move |_| pending_flows_clone.clone()));

        // ONE shared pending-write store for all MCP providers (leases/read-
        // write ruling, increment 4c). Installed on every integration below so
        // all once_only chokepoints and the frontend approve panel coordinate
        // through the same at-most-once state machine. Registered as a DI
        // singleton so the GPUI layer resolves the same handle to render/approve.
        let pending_writes = Arc::new(PendingWriteStore::new());
        let pending_writes_di = pending_writes.clone();
        // fluxdi treats an `Arc<T>`-returning root closure as the shared
        // instance of `T`, so `provide::<PendingWriteStore>` + a closure cloning
        // this Arc registers ONE shared store; `resolve::<PendingWriteStore>`
        // returns that same `Arc<PendingWriteStore>` (mirrors PendingOAuthFlows).
        injector.provide::<PendingWriteStore>(Provider::root(move |_| pending_writes_di.clone()));
        let pending_writes_for_registry = pending_writes.clone();

        let configs_for_registry = configs.clone();

        // Register the registry as an async singleton — resolved in parallel with other
        // DI services.
        injector.provide::<McpIntegrationRegistry>(Provider::root_async(move |resolver| {
            let configs_for_registry = configs_for_registry.clone();
            let pending_flows = pending_flows.clone();
            let pending_writes = pending_writes_for_registry.clone();
            let superseded = superseded.clone();
            let ignored = ignored.clone();
            async move {
                // Every non-connected integration is disclosed on this bus so
                // the resulting blank pages are attributable. A container that
                // registers integrations but no bus can never tell the user
                // anything is wrong, so its absence is a wiring bug, not a mode
                // — fail the boot rather than ship a mute build.
                let degraded_bus: Arc<DegradedSignalBus> = (*resolver
                    .try_resolve_async::<Arc<DegradedSignalBus>>()
                    .await
                    .unwrap_or_else(|e| {
                        panic!(
                            "[McpIntegrationsModule] No DegradedSignalBus in this container ({e}) \
                             — integration connect failures would have no disclosure channel at \
                             all and their pages would render blank. Register it in the \
                             composition root (holon-app `add_frontend`)."
                        )
                    }))
                .clone();

                for s in superseded.iter() {
                    disclose_superseded_sidecar(s, &degraded_bus);
                }
                for s in ignored.iter() {
                    disclose_ignored_sidecar(s, &degraded_bus);
                }

                let db_handle = resolver
                    .resolve_async::<dyn holon::di::DbHandleProvider>()
                    .await
                    .handle();
                let cache_factory = resolver
                    .resolve_async::<dyn holon_core::CacheFactory>()
                    .await;
                let token_store: Arc<dyn SyncTokenStore> =
                    resolver.resolve_async::<dyn SyncTokenStore>().await;
                let type_registry = resolver.resolve::<TypeRegistry>();
                // Boot-ordering gate: provider syncs (initial + notification-
                // driven) wait for the org initial scan to finish before
                // touching the serialized DatabaseActor. Registered in
                // `add_frontend`, opened by the `post_ready` scan barrier.
                let sync_gate: SyncGate = (*resolver.resolve::<SyncGate>()).clone();

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
                            disclose_connect_failure(name, &e, &degraded_bus);
                            continue;
                        }
                        Err(e) => {
                            panic!(
                                "[McpIntegrationsModule] Invalid integration config for provider \
                                 '{name}': {e:#}"
                            );
                        }
                    };

                    let result = build_mcp_integration(
                        mcp_config,
                        db_handle.clone(),
                        cache_factory.clone(),
                        token_store.clone(),
                        &pending_flows,
                        sync_gate.clone(),
                    )
                    .await;

                    match result {
                        Ok(holon_mcp_client::McpConnectionResult::Connected(mut integration)) => {
                            info!(
                                "[McpIntegrationsModule] Provider '{}' connected ({} operations)",
                                name,
                                integration.operation_provider.operations().len()
                            );

                            // Install the shared pending-write store so once_only
                            // writes on this connector coordinate with the frontend
                            // approve panel (leases/read-write ruling, increment 4c).
                            integration.set_pending_store(pending_writes.clone());

                            // Register MCP entity types in TypeRegistry for GQL graph
                            integration.register_entity_types(&type_registry);

                            // Enqueue the initial sync through the integration's
                            // serialized sync event loop (same consumer as
                            // notification resyncs and poll ticks) — doesn't block
                            // startup and can't overlap a concurrent resync.
                            if let Err(e) = integration.request_initial_sync() {
                                warn!(
                                    "[McpIntegrationsModule] Initial sync for '{}' could not be \
                                     enqueued: {e}",
                                    name
                                );
                            }

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
                            disclose_needs_auth(&provider_name, &auth_url, &degraded_bus);
                        }
                        Err(e) => {
                            warn!(
                                "[McpIntegrationsModule] Failed to connect provider '{}': {e}",
                                name
                            );
                            disclose_connect_failure(name, &e, &degraded_bus);
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

        // Register each config's OperationProvider so OperationDispatcher discovers
        // them. Each factory resolves the shared registry and looks its
        // integration up BY NAME — the registry may hold fewer integrations
        // than configs (skipped/failed connections), so positional indexing
        // would misroute operations.
        for (name, _) in configs.iter() {
            let name = name.clone();
            injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
                move |resolver| {
                    let name = name.clone();
                    async move {
                        let registry = resolver.resolve_async::<McpIntegrationRegistry>().await;

                        if let Some(integration) = registry.by_name(&name) {
                            info!(
                                "[McpIntegrationsModule] Registered OperationProvider for \
                                 '{name}' with {} operations",
                                integration.operation_provider.operations().len()
                            );
                            let registry_clone = registry.clone();
                            Arc::new(RegistryOperationProxy {
                                registry: registry_clone,
                                name,
                            }) as Arc<dyn OperationProvider>
                        } else {
                            // No emit here: every route by which a configured
                            // integration can be missing from the registry
                            // (unresolved `${VAR}`, NeedsAuth, connect error)
                            // already disclosed itself on the degraded bus with
                            // the actual cause, which this site does not know.
                            warn!(
                                "[McpIntegrationsModule] Integration '{name}' unavailable (not \
                                 configured or failed to connect) — registering inert provider; \
                                 cause was disclosed on the degraded bus at boot"
                            );
                            Arc::new(EmptyOperationProvider { name }) as Arc<dyn OperationProvider>
                        }
                    }
                },
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use holon::sync::DegradedSignalBus;
    use holon::sync::ShareDegradedReason;

    use super::*;

    /// Drives the REAL sidecar-spawn failure (a `command` that is not on disk)
    /// through the same disclosure the registry factory uses, and asserts the
    /// degraded bus carries the integration name and the connect error.
    #[tokio::test(flavor = "current_thread")]
    async fn dead_sidecar_command_is_disclosed_on_the_degraded_bus() {
        let Err(err) = holon_mcp_client::connect_mcp_child(
            "/nonexistent/holon-test-sidecar",
            &[],
            &HashMap::new(),
        )
        .await
        else {
            panic!("spawning a nonexistent sidecar binary must fail");
        };

        // Disclose BEFORE subscribing — the registry factory runs in boot DI,
        // the only consumer subscribes at window launch.
        let bus = DegradedSignalBus::new();
        disclose_connect_failure("todoist", &err, &bus);

        let mut current = bus.subscribe().current;
        assert_eq!(current.len(), 1);
        let ev = current.remove(0);
        assert_eq!(ev.shared_tree_id, "todoist");
        let ShareDegradedReason::IntegrationConnectFailed { integration, error } = ev.reason else {
            panic!("expected IntegrationConnectFailed, got {:?}", ev.reason);
        };
        assert_eq!(integration, "todoist");
        assert!(
            error.contains("No such file") || error.contains("os error 2"),
            "error must carry the spawn failure: {error}"
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn pending_oauth_is_disclosed_on_the_degraded_bus() {
        let bus = DegradedSignalBus::new();
        disclose_needs_auth("linear", "https://linear.app/oauth/authorize", &bus);

        let mut current = bus.subscribe().current;
        assert_eq!(current.len(), 1);
        let ev = current.remove(0);
        assert_eq!(ev.shared_tree_id, "linear");
        let ShareDegradedReason::IntegrationNeedsAuth {
            integration,
            auth_url,
        } = ev.reason
        else {
            panic!("expected IntegrationNeedsAuth, got {:?}", ev.reason);
        };
        assert_eq!(integration, "linear");
        assert_eq!(auth_url, "https://linear.app/oauth/authorize");
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
                "MCP integration '{}' vanished from the registry — proxy/registry invariant \
                 violated",
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
        params: holon_core::storage::types::StorageEntity,
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
        _: holon_core::storage::types::StorageEntity,
    ) -> holon_core::traits::Result<holon_core::traits::OperationResult> {
        Err(format!(
            "MCP integration '{}' unavailable — not configured or failed to connect at startup",
            self.name
        )
        .into())
    }
}

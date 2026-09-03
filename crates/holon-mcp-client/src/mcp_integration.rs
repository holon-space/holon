use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use holon_api::DynamicEntity;
use holon_core::CacheFactory;
use holon_core::EntityCache;
use holon_core::SyncGate;
use holon_core::SyncTokenStore;
use holon_core::SyncableProvider;
use holon_turso::turso::DbHandle;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::Instrument;
use tracing::error;
use tracing::info;
use tracing::warn;

use crate::credential_store::TursoCredentialStore;
use crate::mcp_notification_handler::NotifyingClientHandler;
use crate::mcp_notification_handler::ResourceUpdateReceiver;
use crate::mcp_provider::EntityFieldReader;
use crate::mcp_provider::McpOperationProvider;
use crate::mcp_provider::McpRunningService;
use crate::mcp_provider::connect_mcp_child_with_handler;
use crate::mcp_provider::connect_mcp_oauth_with_handler;
use crate::mcp_provider::connect_mcp_with_handler;
use crate::mcp_resource_discovery::is_concrete_uri;
use crate::mcp_resource_discovery::parse_resource_template_meta;
use crate::mcp_sidecar::EntityConfig;
use crate::mcp_sidecar::McpSidecar;
use crate::mcp_sidecar::SyncConfig;
use crate::mcp_sidecar::SyncInterval;
use crate::mcp_sync_engine::McpSyncEngine;
use crate::mcp_sync_strategy::SyncStrategy;
use crate::rest_transport::RestCallSurface;
use crate::rest_transport::RestManual;
use crate::sync_freshness::ProbedResourceCapabilities;

/// Default poll cadence for a `rest` integration whose sync entities declare
/// no per-entity `sync.interval` and whose `holon.poll_interval` is
/// unset. REST has no subscription freshness, so an unset interval must not
/// mean "never refresh" — five minutes bounds staleness without hammering.
const DEFAULT_REST_POLL_INTERVAL: Duration = Duration::from_secs(300);

/// Transport configuration for connecting to a data source.
///
/// `Http`/`ChildProcess` reach a server that speaks MCP; `Rest` reaches a plain
/// HTTP/JSON API directly via a UTCP-style manual (same connector engine,
/// served behind the
/// [`McpCallSurface`](crate::mcp_call_surface::McpCallSurface) seam).
#[derive(Debug)]
pub enum McpTransport {
    Http {
        uri: String,
    },
    ChildProcess {
        command: String,
        args: Vec<String>,
        env: HashMap<String, String>,
    },
    Rest {
        manual: RestManual,
        /// Default poll cadence for sync entities that declare no per-entity
        /// `sync.interval`. `None` falls to [`DEFAULT_REST_POLL_INTERVAL`].
        poll_interval: Option<SyncInterval>,
    },
}

/// Authentication mode for MCP HTTP transport.
pub enum AuthMode {
    /// No authentication.
    None,
    /// Static Bearer token (e.g., Todoist API key).
    StaticToken(String),
    /// OAuth 2.1 with persistent credentials in Turso.
    OAuth {
        credential_store: Arc<TursoCredentialStore>,
    },
}

impl std::fmt::Debug for AuthMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthMode::None => write!(f, "None"),
            AuthMode::StaticToken(_) => write!(f, "StaticToken(...)"),
            AuthMode::OAuth { .. } => write!(f, "OAuth {{ .. }}"),
        }
    }
}

/// Configuration for a generic MCP integration.
#[derive(Debug)]
pub struct McpIntegrationConfig {
    pub provider_name: String,
    pub transport: McpTransport,
    pub sidecar_yaml: String,
    /// Authentication mode for HTTP transport.
    pub auth_mode: AuthMode,
}

/// Result of building an MCP integration.
///
/// OAuth connections may require user consent before the connection is ready.
#[allow(clippy::large_enum_variant)] // OAuthPending carries small flow-state; Connected wraps a full integration handle
pub enum McpConnectionResult {
    /// Connection is ready to use.
    Connected(McpIntegration),
    /// OAuth consent needed — frontend must open `auth_url` in a browser,
    /// capture the redirect callback, and call `complete_oauth` with the
    /// authorization code and CSRF state.
    NeedsAuth {
        auth_url: String,
        provider_name: String,
    },
}

/// Result of building an MCP integration: operation provider, sync engine, and
/// running service.
pub struct McpIntegration {
    pub operation_provider: McpOperationProvider,
    pub sync_engine: Arc<McpSyncEngine>,
    /// Must be kept alive for the MCP connection to stay open.
    pub service: McpRunningService,
    /// The single consumer serializing all sync work for this integration:
    /// initial sync, notification resyncs, and poll ticks.
    pub sync_event_task: JoinHandle<()>,
    /// Producers feeding `sync_event_task`: the notification forwarder and
    /// one poll ticker per entity with a configured `sync.interval`.
    /// Held so their lifetime is owned by the integration.
    pub background_tasks: Vec<JoinHandle<()>>,
    /// Server resource capabilities probed from `peer_info()` at connect.
    pub resource_capabilities: ProbedResourceCapabilities,
    /// Cache table names that have an associated FDW table.
    pub fdw_backed_tables: Vec<String>,
    /// Producer handle into the sync event loop.
    sync_event_tx: mpsc::UnboundedSender<SyncEvent>,
}

/// A unit of sync work, serialized through one consumer per integration so
/// notification resyncs, poll ticks, and the initial sync never overlap.
#[derive(Debug)]
pub enum SyncEvent {
    /// Full sync of every sync-configured entity (the initial sync).
    SyncAll,
    /// A `notifications/resources/updated` arrived for this URI.
    NotificationUri(String),
    /// Poll cadence fired for this entity (sidecar `sync.interval`).
    PollTick(String),
}

impl McpIntegration {
    /// Enqueue the initial full sync through the serialized sync event loop.
    /// Returns an error if the loop has already stopped (channel closed).
    pub fn request_initial_sync(&self) -> anyhow::Result<()> {
        self.sync_event_tx.send(SyncEvent::SyncAll).map_err(|_| {
            anyhow::anyhow!("sync event loop is not running — cannot enqueue initial sync")
        })
    }

    /// Install the shared pending-write store on this integration's operation
    /// provider (leases/read-write ruling, increment 4c). The DI composition
    /// root creates ONE store and calls this on every integration so all
    /// once_only chokepoints and the frontend approve panel coordinate through
    /// the same at-most-once state machine.
    pub fn set_pending_store(&mut self, store: crate::write_authorization::SharedPendingWrites) {
        self.operation_provider.set_pending_store(store);
    }

    /// Register all entity types from the sidecar config into the TypeRegistry.
    /// Called by frontends after building the integration so GQL graph includes
    /// MCP entities.
    pub fn register_entity_types(
        &self,
        type_registry: &holon_profiles::TypeRegistry,
    ) -> anyhow::Result<()> {
        register_sidecar_entity_types(
            self.sync_engine.sidecar(),
            self.sync_engine.provider_name(),
            type_registry,
        )
    }
}

/// Refuse two connectors that would answer to the same canonical entity name.
///
/// Entity names are the namespace ACROSS integrations, and the prefix is part
/// of the name: `entity_prefix: todoist_` with key `tasks` canonicalizes to the
/// same `todoist-tasks` as an unprefixed key `todoist_tasks` in another
/// sidecar. Two connectors answering one name means a dispatch lands in
/// whichever the routing scan reaches first; registration alone would not catch
/// it, because it refuses on the (entity, op) PAIR and two connectors naming
/// their tools differently overlap on no pair.
///
/// Checked over every declared entity, including schema-less ones: those
/// register no type, so the type registry's own collision check never sees
/// them, yet their tools still produce descriptors.
pub fn assert_no_cross_sidecar_entity_collisions<'a, I, K>(sidecars: I) -> anyhow::Result<()>
where
    I: IntoIterator<Item = (&'a str, Option<&'a str>, K)>,
    K: IntoIterator<Item = &'a str>,
{
    let mut claimed: HashMap<String, (String, String)> = HashMap::new();
    for (provider, prefix, keys) in sidecars {
        for key in keys {
            let canonical = crate::canonical_entity_name(prefix, key)
                .as_str()
                .to_string();
            if let Some((other_provider, other_key)) =
                claimed.insert(canonical.clone(), (provider.to_string(), key.to_string()))
            {
                anyhow::bail!(
                    "integrations '{other_provider}' (entity '{other_key}') and '{provider}' \
                     (entity '{key}') both resolve to entity '{canonical}'. An entity name is \
                     the identity ACROSS integrations — give one of them a distinct \
                     `entity_prefix` or entity name."
                );
            }
        }
    }
    Ok(())
}

/// Register every entity a sidecar declares as a type in `type_registry`.
///
/// Free-standing so a test harness standing in for a connector seeds the
/// registry through the very same code the connected integration runs — a
/// second copy would let the two drift, and the drift is invisible until a
/// registry consumer behaves differently under test than in the app.
///
/// A name another owner already holds is an ERROR, not a warning:
/// `TypeRegistry::register` inserts over whatever is there, so a sidecar
/// entity colliding with `block` — or with a second connector's entity —
/// would silently replace that type's definition and every consumer reading
/// the registry would quietly change shape. Re-registering the SAME
/// provider's entity is a no-op, so a reconnect is not an error.
pub fn register_sidecar_entity_types(
    sidecar: &McpSidecar,
    provider_name: &str,
    type_registry: &holon_profiles::TypeRegistry,
) -> anyhow::Result<()> {
    for (entity_name, entity_config) in &sidecar.entities {
        let table_name = sidecar.prefixed_name(entity_name).table_name();
        let Some(td) = entity_config.to_type_definition(
            &table_name,
            provider_name,
            sidecar.write_ownership(entity_name),
        ) else {
            continue; // schema-less entity: nothing to register
        };
        if let Some(existing) = type_registry.get(&table_name) {
            // By ORIGIN, not by write authority: a mirror of a feed the
            // connector only reads is still that connector's definition.
            match existing.declaring_integration() {
                Some(owner) if owner == provider_name => continue,
                Some(owner) => anyhow::bail!(
                    "provider '{provider_name}': entity '{entity_name}' registers type \
                     '{table_name}', which integration '{owner}' already owns. Registering it \
                     would replace that integration's definition and silently change the shape \
                     every reader sees. Give one of the two an `entity_prefix` or a distinct \
                     entity name."
                ),
                None => anyhow::bail!(
                    "provider '{provider_name}': entity '{entity_name}' registers type \
                     '{table_name}', which is already a built-in or user-defined type. \
                     Registering it would replace that definition and silently change the shape \
                     every reader sees. Give the sidecar an `entity_prefix` or rename the entity."
                ),
            }
        }
        // The name is free — the collision check above is what decides that,
        // and it is the only guard, because `register` itself inserts over
        // whatever it finds and reports success either way.
        type_registry
            .register(td)
            .expect("TypeRegistry::register accepts every definition");
    }
    Ok(())
}

/// State parked between `build_mcp_integration` returning `NeedsAuth` and
/// the frontend calling `complete_oauth` with the authorization code.
struct PendingOAuth {
    auth_manager: rmcp::transport::auth::AuthorizationManager,
    uri: String,
    sidecar: McpSidecar,
    db_handle: DbHandle,
    cache_factory: Arc<dyn CacheFactory>,
    token_store: Arc<dyn SyncTokenStore>,
    provider_name: String,
    sync_gate: SyncGate,
}

/// Registry of in-flight OAuth flows awaiting user consent.
///
/// Keyed by provider_name (the MCP server URI). Thread-safe for access
/// from both the integration builder and the FFI completion call.
#[derive(Default)]
pub struct PendingOAuthFlows {
    flows: Mutex<HashMap<String, PendingOAuth>>,
}

impl PendingOAuthFlows {
    pub fn new() -> Self {
        Self::default()
    }

    async fn insert(&self, key: String, pending: PendingOAuth) {
        self.flows.lock().await.insert(key, pending);
    }

    async fn take(&self, key: &str) -> Option<PendingOAuth> {
        self.flows.lock().await.remove(key)
    }

    /// Complete an OAuth flow after the frontend captured the authorization
    /// code.
    ///
    /// Exchanges the code for a token, connects to the MCP server, and returns
    /// the fully-wired `McpIntegration`.
    pub async fn complete_oauth(
        &self,
        provider_name: &str,
        code: &str,
        state: &str,
    ) -> anyhow::Result<McpIntegration> {
        let pending = self.take(provider_name).await.ok_or_else(|| {
            anyhow::anyhow!(
                "No pending OAuth flow for provider '{provider_name}'. Was build_mcp_integration \
                 called first?"
            )
        })?;

        info!(
            "[OAuth] Completing flow for '{}', exchanging code for token...",
            pending.uri
        );
        pending
            .auth_manager
            .exchange_code_for_token(code, state)
            .await
            .map_err(|e| anyhow::anyhow!("OAuth token exchange failed: {e}"))?;

        info!("[OAuth] Token exchange successful, connecting...");
        let (handler, receiver) = NotifyingClientHandler::new();
        let (peer, service) =
            connect_mcp_oauth_with_handler(&pending.uri, pending.auth_manager, handler).await?;
        finish_integration(
            peer,
            service,
            pending.sidecar,
            pending.db_handle,
            pending.cache_factory,
            pending.token_store,
            pending.provider_name,
            receiver,
            pending.sync_gate,
        )
        .await
    }
}

/// Build a complete MCP integration from config.
///
/// For OAuth connections without stored credentials, returns
/// `McpConnectionResult::NeedsAuth`. The frontend should:
/// 1. Open `auth_url` in a browser (e.g., via `flutter_web_auth_2`)
/// 2. Capture the redirect callback URL containing `?code=...&state=...`
/// 3. Call `pending_flows.complete_oauth(provider_name, code, state)`
pub async fn build_mcp_integration(
    config: McpIntegrationConfig,
    db_handle: DbHandle,
    cache_factory: Arc<dyn CacheFactory>,
    token_store: Arc<dyn SyncTokenStore>,
    pending_flows: &PendingOAuthFlows,
    sync_gate: SyncGate,
) -> anyhow::Result<McpConnectionResult> {
    let sidecar = McpSidecar::from_yaml(&config.sidecar_yaml)?;

    match &config.transport {
        McpTransport::Http { uri } => match &config.auth_mode {
            AuthMode::None => {
                let (handler, receiver) = NotifyingClientHandler::new();
                let (peer, service) = connect_mcp_with_handler(uri, None, handler).await?;
                let integration = finish_integration(
                    peer,
                    service,
                    sidecar,
                    db_handle,
                    cache_factory,
                    token_store,
                    config.provider_name,
                    receiver,
                    sync_gate,
                )
                .await?;
                Ok(McpConnectionResult::Connected(integration))
            }
            AuthMode::StaticToken(token) => {
                let (handler, receiver) = NotifyingClientHandler::new();
                let (peer, service) =
                    connect_mcp_with_handler(uri, Some(token.as_str()), handler).await?;
                let integration = finish_integration(
                    peer,
                    service,
                    sidecar,
                    db_handle,
                    cache_factory,
                    token_store,
                    config.provider_name,
                    receiver,
                    sync_gate,
                )
                .await?;
                Ok(McpConnectionResult::Connected(integration))
            }
            AuthMode::OAuth { credential_store } => {
                build_oauth_integration(
                    uri.clone(),
                    credential_store.clone(),
                    sidecar,
                    db_handle,
                    cache_factory,
                    token_store,
                    config.provider_name,
                    pending_flows,
                    sync_gate,
                )
                .await
            }
        },
        McpTransport::ChildProcess { command, args, env } => {
            let (handler, receiver) = NotifyingClientHandler::new();
            let (peer, service) =
                connect_mcp_child_with_handler(command, args, env, handler).await?;
            let integration = finish_integration(
                peer,
                service,
                sidecar,
                db_handle,
                cache_factory,
                token_store,
                config.provider_name,
                receiver,
                sync_gate,
            )
            .await?;
            Ok(McpConnectionResult::Connected(integration))
        }
        McpTransport::Rest {
            manual,
            poll_interval,
        } => {
            // The `rest` transport shares the whole connector read path
            // (`SyncStrategy`/`McpCallSurface`) with MCP, but has no peer and no
            // resource subscriptions — it is driven by a poll-only background
            // runner. Leases / read-write / vtable-write-through are out of
            // scope and fail loud inside `finish_rest_integration`.
            let integration = finish_rest_integration(
                manual.clone(),
                *poll_interval,
                sidecar,
                db_handle,
                cache_factory,
                token_store,
                config.provider_name,
                sync_gate,
            )
            .await?;
            Ok(McpConnectionResult::Connected(integration))
        }
    }
}

/// Attempt OAuth connection: use stored tokens if available, otherwise return
/// NeedsAuth.
#[allow(clippy::too_many_arguments)] // each arg is a distinct subsystem
async fn build_oauth_integration(
    uri: String,
    credential_store: Arc<TursoCredentialStore>,
    sidecar: McpSidecar,
    db_handle: DbHandle,
    cache_factory: Arc<dyn CacheFactory>,
    token_store: Arc<dyn SyncTokenStore>,
    provider_name: String,
    pending_flows: &PendingOAuthFlows,
    sync_gate: SyncGate,
) -> anyhow::Result<McpConnectionResult> {
    use rmcp::transport::auth::AuthorizationManager;

    let mut auth_manager = AuthorizationManager::new(&uri)
        .await
        .map_err(|e| anyhow::anyhow!("OAuth metadata discovery failed for '{uri}': {e}"))?;
    auth_manager.set_credential_store((*credential_store).clone());

    let has_stored = auth_manager
        .initialize_from_store()
        .await
        .map_err(|e| anyhow::anyhow!("Failed to load stored OAuth credentials: {e}"))?;

    if has_stored {
        info!("[OAuth] Found stored credentials for '{uri}', attempting connection");
        let (handler, receiver) = NotifyingClientHandler::new();
        let (peer, service) = connect_mcp_oauth_with_handler(&uri, auth_manager, handler).await?;
        let integration = finish_integration(
            peer,
            service,
            sidecar,
            db_handle,
            cache_factory,
            token_store,
            provider_name,
            receiver,
            sync_gate,
        )
        .await?;
        return Ok(McpConnectionResult::Connected(integration));
    }

    info!("[OAuth] No stored credentials for '{uri}', initiating OAuth flow");

    // Use a custom URL scheme for flutter_web_auth_2 callback interception.
    // The OS hands the redirect URL back to Flutter without needing a localhost
    // server.
    let redirect_uri = "holon://oauth/callback";
    let client_config = auth_manager
        .register_client("holon", redirect_uri)
        .await
        .map_err(|e| anyhow::anyhow!("OAuth dynamic client registration failed: {e}"))?;
    auth_manager
        .configure_client(client_config)
        .map_err(|e| anyhow::anyhow!("Failed to configure OAuth client: {e}"))?;

    let auth_url = auth_manager
        .get_authorization_url(&[])
        .await
        .map_err(|e| anyhow::anyhow!("Failed to get OAuth authorization URL: {e}"))?;

    // Park the auth manager so complete_oauth can finish the flow later
    let key = provider_name.clone();
    pending_flows
        .insert(
            key,
            PendingOAuth {
                auth_manager,
                uri,
                sidecar,
                db_handle,
                cache_factory,
                token_store,
                provider_name: provider_name.clone(),
                sync_gate,
            },
        )
        .await;

    Ok(McpConnectionResult::NeedsAuth {
        auth_url,
        provider_name,
    })
}

/// Strategies that built successfully, paired with the per-entity failures
/// that were disclosed rather than fatal.
type EntityStrategyBuild = (
    HashMap<String, Box<dyn SyncStrategy>>,
    Vec<(String, anyhow::Error)>,
);

/// Common integration finalization: build caches, discover resources, build
/// strategies, subscribe.
/// Build a sync strategy for every entity that declares a `SyncConfig`,
/// collecting per-entity failures instead of aborting on the first one.
///
/// Returns `(strategies, failures)`. A failure means that entity will not
/// sync, but the rest of the integration is unaffected — this is the
/// disclosed-degradation contract that keeps a single bad (often
/// auto-discovered) entity from taking down the whole integration.
fn build_entity_strategies(entities: &HashMap<String, EntityConfig>) -> EntityStrategyBuild {
    let mut strategies: HashMap<String, Box<dyn SyncStrategy>> = HashMap::new();
    let mut failures: Vec<(String, anyhow::Error)> = Vec::new();
    for (entity_name, entity_config) in entities {
        let Some(ref sync_config) = entity_config.sync else {
            continue;
        };
        match sync_config.into_strategy() {
            Ok(strategy) => {
                strategies.insert(entity_name.clone(), strategy);
            }
            Err(err) => failures.push((entity_name.clone(), err)),
        }
    }
    (strategies, failures)
}

#[allow(clippy::too_many_arguments)] // each arg is a distinct subsystem
async fn finish_integration(
    peer: rmcp::service::Peer<rmcp::RoleClient>,
    service: McpRunningService,
    mut sidecar: McpSidecar,
    db_handle: DbHandle,
    cache_factory: Arc<dyn CacheFactory>,
    token_store: Arc<dyn SyncTokenStore>,
    provider_name: String,
    receiver: ResourceUpdateReceiver,
    sync_gate: SyncGate,
) -> anyhow::Result<McpIntegration> {
    // Auto-discover entities from resource templates
    let templates = peer
        .list_all_resource_templates()
        .await
        .unwrap_or_else(|e| {
            warn!("[finish_integration] Failed to list resource templates: {e}");
            vec![]
        });

    for template in &templates {
        if let Some(meta) = parse_resource_template_meta(template) {
            let id_column = meta.primary_keys.first().cloned().unwrap_or("id".into());

            // Match by direct key name first, then by source_name mapping
            let yaml_key = if sidecar.entities.contains_key(&meta.entity_name) {
                Some(meta.entity_name.clone())
            } else {
                sidecar
                    .find_key_by_source_name(&meta.entity_name)
                    .map(|k| k.to_string())
            };

            if let Some(yaml_key) = yaml_key {
                let existing = sidecar.entities.get_mut(&yaml_key).unwrap();
                if existing.schema.is_empty() {
                    info!(
                        "[finish_integration] Merging auto-discovered schema into sidecar entity \
                         '{}' (source: '{}')",
                        yaml_key, meta.entity_name
                    );
                    existing.schema = meta.fields;
                }
                if existing.id_column.is_none() {
                    existing.id_column = Some(id_column);
                }
                continue;
            }

            let short_name = meta.entity_name.clone();

            // Only concrete templates back a standalone list sync. A
            // parameterized template (e.g. `.../{project_id}/plan`) has no
            // parent value here, so it is registered as a schema-only entity
            // (cache table, reachable via parent fan-out) rather than given an
            // unbuildable `list_resource` strategy — which previously aborted
            // the WHOLE integration at `into_strategy` (BugFunnel row 27).
            let sync = if is_concrete_uri(&meta.uri_template) {
                info!(
                    "[finish_integration] Auto-discovered entity '{}' from resource template '{}'",
                    meta.entity_name, meta.uri_template
                );
                Some(SyncConfig {
                    list_tool: None,
                    extract_path: None,
                    list_params: HashMap::new(),
                    cursor: None,
                    list_resource: Some(meta.uri_template),
                    uri_params: HashMap::new(),
                    interval: None,
                    project: HashMap::new(),
                })
            } else {
                info!(
                    "[finish_integration] Auto-discovered entity '{}' from PARAMETERIZED template \
                     '{}' — registered schema-only (no standalone list sync; needs a parent key)",
                    meta.entity_name, meta.uri_template
                );
                None
            };

            sidecar.entities.insert(
                meta.entity_name.clone(),
                EntityConfig {
                    short_name: Some(short_name),
                    source_name: None,
                    id_column: Some(id_column),
                    schema: meta.fields,
                    sync,
                    vtable: None,
                    profile_variants: Vec::new(),
                },
            );
        }
    }

    // Fail loud on a sync-vs-write_through clash: the engine's in-memory mirror
    // assumes it is the SOLE writer to a sync entity's cache table (it keeps the
    // mirror consistent by write-through after each committed batch). A
    // `vtable.write_through` entity has the FDW cursor writing the same table for
    // IVM — a second, unobserved writer that would silently desync the mirror.
    // These two mechanisms must never target the same cache table.
    for (entity_name, entity_config) in &sidecar.entities {
        let has_sync = entity_config.sync.is_some();
        let has_write_through = entity_config
            .vtable
            .as_ref()
            .is_some_and(|v| v.write_through);
        if has_sync && has_write_through {
            let table = sidecar.prefixed_name(entity_name).table_name();
            anyhow::bail!(
                "provider '{provider_name}': entity '{entity_name}' declares both a `sync` \
                 strategy and `vtable.write_through` on cache table '{table}'. The sync engine's \
                 in-memory mirror requires the engine to be the sole writer of a sync entity's \
                 table; a write-through FDW cursor on the same table would desync it. Split them \
                 into separate entities/tables or drop one."
            );
        }
    }

    // Build caches and strategies.
    let (caches, entity_readers) =
        build_entity_caches(&sidecar, &provider_name, &cache_factory).await?;

    // Build sync strategies with disclosed degradation: one entity whose
    // `SyncConfig` cannot form a strategy is skipped and reported loudly, so a
    // single bad entity (e.g. an auto-discovered one) never sinks the whole
    // integration. The declared, working entities survive (BugFunnel row 27).
    let (strategies, strategy_failures) = build_entity_strategies(&sidecar.entities);
    for (entity_name, err) in &strategy_failures {
        error!(
            "[finish_integration] Entity '{entity_name}' will NOT sync — failed to build strategy: \
             {err:#}. Integration '{provider_name}' still connects (disclosed degradation)."
        );
    }

    // Register foreign tables for entities with vtable config.
    // Entity → id column map so enumerate_from fan-out knows which parent
    // columns carry scheme-prefixed ids (strip boundary in mcp_vtable).
    let enumeration_id_columns: std::collections::HashMap<String, String> = sidecar
        .entities
        .iter()
        .map(|(name, cfg)| (name.clone(), cfg.id_column_or_default()))
        .collect();
    let mut fdw_backed_tables = Vec::new();
    let mut identity_less_fdw_tables: Vec<String> = Vec::new();
    let mut contractless_fdw_tables: Vec<String> = Vec::new();
    for (entity_name, entity_config) in &sidecar.entities {
        if let Some(ref vtable_config) = entity_config.vtable {
            let table_name = sidecar.prefixed_name(entity_name).table_name();
            let columns: Vec<(String, String)> = entity_config
                .schema
                .iter()
                .map(|f| (f.name.clone(), f.sql_type.clone()))
                .collect();

            if columns.is_empty() {
                warn!(
                    "[finish_integration] Entity '{}' has vtable config but no schema — skipping \
                     foreign table",
                    entity_name
                );
                continue;
            }
            if entity_config.identity_columns().is_empty() {
                identity_less_fdw_tables.push(format!("{table_name}_fdw"));
            }
            if vtable_config.fetch_contract.is_none() {
                contractless_fdw_tables.push(format!("{table_name}_fdw"));
            }

            // ID scheme: prefix ID column values with "{scheme}:" to match McpSyncEngine.
            // Uses EntityName::as_str() (hyphens) not table_name() (underscores).
            let id_col = entity_config.id_column_or_default();
            let entity_type = sidecar.prefixed_name(entity_name);
            let id_scheme = Some((id_col, entity_type.as_str().to_string()));

            // If write_through is enabled, pass the cache table name so the cursor
            // writes fetched rows back for IVM. The cache table is created by
            // the CacheFactory for any entity with a schema — sync is not required.
            let cache_table = if vtable_config.write_through {
                Some(table_name.clone())
            } else {
                None
            };

            let fdw = Arc::new(crate::mcp_vtable::McpForeignDataWrapper::new(
                &table_name,
                &columns,
                vtable_config,
                Arc::new(peer.clone()),
                id_scheme,
                &entity_config.identity_columns(),
                cache_table,
                tokio::runtime::Handle::current(),
                sidecar.entity_prefix.as_deref(),
                &enumeration_id_columns,
            ));

            // Suffix with _fdw to distinguish from the cache table
            let fdw_table_name = format!("{table_name}_fdw");
            db_handle
                .register_foreign_table(&fdw_table_name, fdw)
                .await
                .with_context(|| {
                    format!(
                        "[finish_integration] Failed to register foreign table '{fdw_table_name}'"
                    )
                })?;
            info!(
                "[finish_integration] Registered foreign table '{}' for entity '{}'",
                fdw_table_name, entity_name
            );
            if vtable_config.write_through {
                fdw_backed_tables.push(table_name.clone());
            }
        }
    }

    reject_unmaintainable_fdw_queries(
        &sidecar,
        &identity_less_fdw_tables,
        &contractless_fdw_tables,
        &provider_name,
    )?;

    // Sidecar-declared derived views (generic): the cache tables they select
    // from were just created by the CacheFactory above.
    reconcile_sidecar_views(&sidecar, &db_handle, &provider_name).await?;

    let operation_provider =
        McpOperationProvider::from_peer_shared(peer.clone(), sidecar.clone(), entity_readers)
            .await?;

    // Build vtable subscriptions for FDW-backed entities
    let vtable_subs: Vec<crate::mcp_sync_engine::VtableSubscription> = sidecar
        .entities
        .iter()
        .filter_map(|(name, config)| {
            let vt = config.vtable.as_ref()?;
            let template = vt.list_resource.as_ref()?;
            let table_name = sidecar.prefixed_name(name).table_name();
            let params: Vec<String> = vt
                .uri_params
                .iter()
                .filter(|(_, v)| v.is_dynamic())
                .map(|(k, _)| k.clone())
                .collect();
            Some(crate::mcp_sync_engine::VtableSubscription {
                uri_template: template.clone(),
                fdw_table: format!("{table_name}_fdw"),
                param_columns: params,
            })
        })
        .collect();

    // Capability probe: what freshness mechanisms does this server support?
    let resource_capabilities = ProbedResourceCapabilities::from_server(
        peer.peer_info()
            .and_then(|i| i.capabilities.resources.as_ref()),
    );

    // Entities that explicitly opted into polling via `sync.interval`.
    let poll_entities: Vec<(String, std::time::Duration)> = sidecar
        .entities
        .iter()
        .filter_map(|(name, config)| {
            let interval = config.sync.as_ref()?.interval?;
            Some((name.clone(), interval.duration()))
        })
        .collect();

    let sync_engine = Arc::new(McpSyncEngine::new(
        Arc::new(peer.clone()),
        Some(peer),
        strategies,
        caches,
        token_store,
        provider_name.clone(),
        sidecar.clone(),
        vtable_subs,
        Some(db_handle),
    ));

    // Probe-gated subscribe: only attempt `resources/subscribe` against
    // servers that advertise the capability. Against a capable server, an
    // individual subscribe failure is a real error — fail loud.
    if sync_engine.has_subscriptions() {
        if resource_capabilities.subscribe {
            sync_engine.subscribe_all().await.with_context(|| {
                format!(
                    "provider '{provider_name}': server advertises resources.subscribe but \
                     subscribing failed"
                )
            })?;
        } else if poll_entities.is_empty() {
            warn!(
                "provider {provider_name}: no resources.subscribe capability and no sync.interval \
                 configured — caches will be STALE after the initial sync (add `sync: {{ \
                 interval: 60s }}` to entities in the sidecar YAML to poll)"
            );
        } else {
            let cadences: Vec<String> = poll_entities
                .iter()
                .map(|(name, d)| format!("{name}@{}s", d.as_secs()))
                .collect();
            warn!(
                "provider {provider_name}: no resources.subscribe — falling back to polling at {}",
                cadences.join(", ")
            );
        }
    }

    Ok(spawn_runner(
        operation_provider,
        sync_engine,
        service,
        resource_capabilities,
        fdw_backed_tables,
        poll_entities,
        Some(receiver),
        sync_gate,
    ))
}

/// Build the per-entity caches and their field readers from the sidecar. Table
/// names and ID schemes use prefixed names (e.g. "cc_session"); the returned
/// maps are keyed by original entity name (e.g. "session"). Shared by the MCP
/// and `rest` finalizers.
async fn build_entity_caches(
    sidecar: &McpSidecar,
    provider_name: &str,
    cache_factory: &Arc<dyn CacheFactory>,
) -> anyhow::Result<(
    HashMap<String, Arc<dyn EntityCache<DynamicEntity>>>,
    HashMap<String, Arc<dyn EntityFieldReader>>,
)> {
    let mut caches: HashMap<String, Arc<dyn EntityCache<DynamicEntity>>> = HashMap::new();
    let mut entity_readers: HashMap<String, Arc<dyn EntityFieldReader>> = HashMap::new();

    for (entity_name, entity_config) in &sidecar.entities {
        let entity = sidecar.prefixed_name(entity_name);
        let table_name = entity.table_name();
        if let Some(td) = entity_config.to_type_definition(
            &table_name,
            provider_name,
            sidecar.write_ownership(entity_name),
        ) {
            let cache = cache_factory
                .create_dynamic_cache(td)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            entity_readers.insert(
                entity_name.clone(),
                Arc::new(DynamicEntityFieldReader(cache.clone())) as Arc<dyn EntityFieldReader>,
            );
            caches.insert(entity_name.clone(), cache);
        }
    }
    Ok((caches, entity_readers))
}

/// Refuse a sidecar whose SQL reads an `_fdw` table that cannot be maintained.
///
/// Reading the foreign table directly is what puts a view on the mirror, and
/// two declarations decide whether that mirror can be kept correct: a row
/// IDENTITY (without it the engine keeps snapshot semantics and builds no
/// mirror, so the view fills once and is then silently frozen) and a FETCH
/// CONTRACT (without it nothing states how much of a re-fetch's absence means
/// deletion, which is the one unknown whose wrong guess loses data silently).
/// Both are fully known at connect, so both fail here.
///
/// INERT against the shipped sidecars today: every `live_query` and view reads
/// a CACHE table, and the only `_fdw` mentions in `claude-history.yaml` are
/// comments, which never reach this scan. It is a guard for the retarget,
/// proven by fixtures rather than by production config. It also sees sidecar
/// SQL only — a `live_query` authored in the user's vault is out of its reach,
/// which is the likeliest place a real `_fdw` reader would appear first.
fn reject_unmaintainable_fdw_queries(
    sidecar: &McpSidecar,
    identity_less: &[String],
    contractless: &[String],
    provider_name: &str,
) -> anyhow::Result<()> {
    if identity_less.is_empty() && contractless.is_empty() {
        return Ok(());
    }
    // Render DSL and view SQL both carry table names as plain identifiers, so
    // one token scan covers both without a dialect parser.
    let mut sources: Vec<(String, &str)> = sidecar
        .views
        .iter()
        .map(|v| (format!("view '{}'", v.name), v.sql.as_str()))
        .collect();
    for (entity_name, entity) in &sidecar.entities {
        for variant in &entity.profile_variants {
            sources.push((
                format!("entity '{entity_name}' render '{}'", variant.name),
                variant.render.as_str(),
            ));
        }
    }

    for (origin, text) in sources {
        for table in identity_less {
            if names_identifier(text, table) {
                anyhow::bail!(
                    "provider '{provider_name}': {origin} queries foreign table '{table}', whose \
                     entity declares no row identity (no `primary_key` column, no `id_column`, no \
                     column named `id`). The engine keeps such a table on snapshot semantics and \
                     builds no mirror, so this query's view would fill once and then silently \
                     never update. Declare the entity's key column `primary_key: true`, or read \
                     the cache table instead."
                );
            }
        }
        for table in contractless {
            if names_identifier(text, table) {
                anyhow::bail!(
                    "provider '{provider_name}': {origin} queries foreign table '{table}', whose \
                     entity declares no `vtable.fetch_contract`. A view on the mirror is \
                     maintained from re-fetches, and what a re-fetch's ABSENCE means — deleted, \
                     or merely outside the scope this fetch chose — is a property of the source \
                     that cannot be inferred, only declared. Set `fetch_contract` to `snapshot`, \
                     `scoped_snapshot`, or `delta`."
                );
            }
        }
    }
    Ok(())
}

/// Whether `text` mentions `identifier` as a whole SQL identifier rather than
/// as part of a longer name.
fn names_identifier(text: &str, identifier: &str) -> bool {
    let is_ident_char = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(identifier).any(|(at, _)| {
        let before_ok = text[..at]
            .chars()
            .next_back()
            .is_none_or(|c| !is_ident_char(c));
        let after_ok = text[at + identifier.len()..]
            .chars()
            .next()
            .is_none_or(|c| !is_ident_char(c));
        before_ok && after_ok
    })
}

/// Reconcile every sidecar-declared derived view into a materialized view. A
/// view that fails DDL is a hard, loud config error naming the view and the
/// provider — never skip-and-continue (parse, don't validate, at connect).
/// Shared by the MCP and `rest` finalizers.
async fn reconcile_sidecar_views(
    sidecar: &McpSidecar,
    db_handle: &DbHandle,
    provider_name: &str,
) -> anyhow::Result<()> {
    for view in &sidecar.views {
        let view_name = sidecar.prefixed_name(&view.name).table_name();
        holon_turso::matview_manager::reconcile_named_view(db_handle, &view_name, &view.sql)
            .await
            .with_context(|| {
                format!(
                    "sidecar view '{}' of provider '{provider_name}': CREATE MATERIALIZED VIEW \
                     '{view_name}' failed — fix the `views:` SQL in the provider's sidecar YAML \
                     (IVM dialect: single-level GROUP BY aggregates incl. substr(MAX(ts || '|' || \
                     col), N); no correlated subqueries, self-joins, or non-equijoin LEFT JOINs)",
                    view.name
                )
            })?;
        info!(
            "[finish_integration] Sidecar view '{}' ready as matview '{}'",
            view.name, view_name
        );
    }
    Ok(())
}

/// Wire the serialized sync-event loop, notification forwarder, and poll
/// tickers into a finished [`McpIntegration`]. Shared tail of both finalizers:
/// the MCP path passes `Some(receiver)` (server pushes `resources/updated`);
/// the poll-only `rest` path passes `None` (no subscriptions, no
/// notifications).
#[allow(clippy::too_many_arguments)] // assembles the finished integration from its already-built parts
fn spawn_runner(
    operation_provider: McpOperationProvider,
    sync_engine: Arc<McpSyncEngine>,
    service: McpRunningService,
    resource_capabilities: ProbedResourceCapabilities,
    fdw_backed_tables: Vec<String>,
    poll_entities: Vec<(String, Duration)>,
    notification_receiver: Option<ResourceUpdateReceiver>,
    sync_gate: SyncGate,
) -> McpIntegration {
    // One serialized consumer per integration: initial sync, notification
    // resyncs, and poll ticks all flow through the same channel, so per-entity
    // sync work never overlaps.
    let (sync_event_tx, sync_event_rx) = mpsc::unbounded_channel::<SyncEvent>();
    let sync_event_task = spawn_sync_event_loop(
        sync_event_rx,
        sync_engine.clone(),
        sync_gate,
        SyncLoopTuning::default(),
    );

    let mut background_tasks = Vec::new();

    // Notification forwarder: resource-updated URIs -> serialized consumer.
    // Only for transports that push notifications (MCP); `rest` polls instead.
    if let Some(mut receiver) = notification_receiver {
        let tx = sync_event_tx.clone();
        background_tasks.push(tokio::spawn(async move {
            while let Some(uri) = receiver.0.recv().await {
                if tx.send(SyncEvent::NotificationUri(uri)).is_err() {
                    break;
                }
            }
        }));
    }

    // Poll tickers: one per entity with a configured interval.
    for (entity_name, every) in poll_entities {
        let tx = sync_event_tx.clone();
        background_tasks.push(tokio::spawn(async move {
            let mut tick = tokio::time::interval(every);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            // The first tick fires immediately; the initial SyncAll covers it.
            tick.tick().await;
            loop {
                tick.tick().await;
                if tx.send(SyncEvent::PollTick(entity_name.clone())).is_err() {
                    break;
                }
            }
        }));
    }

    McpIntegration {
        operation_provider,
        sync_engine,
        service,
        sync_event_task,
        background_tasks,
        resource_capabilities,
        fdw_backed_tables,
        sync_event_tx,
    }
}

/// Reject sidecar shapes that the `rest` transport cannot serve: `vtable`
/// (needs an MCP peer to back the FDW cursor) and `sync.list_resource` (REST
/// serves GET *calls*, not MCP resources). Pure so it is unit-testable without
/// standing up a DbHandle/CacheFactory.
fn reject_rest_out_of_scope(sidecar: &McpSidecar, provider_name: &str) -> anyhow::Result<()> {
    for (entity_name, entity_config) in &sidecar.entities {
        if let Some(vtable) = &entity_config.vtable {
            anyhow::bail!(
                "provider '{provider_name}': entity '{entity_name}' declares a `vtable` on the \
                 `rest` transport, but REST has no MCP peer to back an FDW cursor \
                 (write_through={}). vtable/write-through are out of scope for `rest`.",
                vtable.write_through
            );
        }
        if let Some(sync) = &entity_config.sync
            && sync.list_resource.is_some()
        {
            anyhow::bail!(
                "provider '{provider_name}': entity '{entity_name}' syncs via `list_resource` \
                 on a `utcp:` connection, but a plain HTTP API serves GET *tools*, not MCP \
                 resources — use `sync.list_tool` naming a tool the manual declares under `{}` \
                 instead.",
                crate::integration_config::MANUAL_TOOLS_KEY
            );
        }
    }
    Ok(())
}

/// Finalize a `rest`-transport integration: a poll-only background runner over
/// the shared connector read path, with no MCP peer and no resource
/// subscriptions.
///
/// Out of scope (fails loud): leases, read-write operations, and
/// `vtable.write_through`. The `rest` transport serves *calls*, not MCP
/// resources, so an entity that syncs via `list_resource` is also rejected.
#[allow(clippy::too_many_arguments)] // mirrors finish_integration; each arg is a distinct subsystem
async fn finish_rest_integration(
    manual: RestManual,
    poll_interval: Option<SyncInterval>,
    sidecar: McpSidecar,
    db_handle: DbHandle,
    cache_factory: Arc<dyn CacheFactory>,
    token_store: Arc<dyn SyncTokenStore>,
    provider_name: String,
    sync_gate: SyncGate,
) -> anyhow::Result<McpIntegration> {
    // Reject the out-of-scope shapes up front (parse, don't validate): the REST
    // runner is read-only and poll-based, so vtable/write_through and
    // resource-based sync are configuration errors, not degraded modes.
    reject_rest_out_of_scope(&sidecar, &provider_name)?;

    let surface: Arc<dyn crate::mcp_call_surface::McpCallSurface> =
        Arc::new(RestCallSurface::new(manual));

    // Build caches + readers, then strategies (disclosed degradation on a bad
    // entity, same as the MCP path).
    let (caches, entity_readers) =
        build_entity_caches(&sidecar, &provider_name, &cache_factory).await?;
    let (strategies, strategy_failures) = build_entity_strategies(&sidecar.entities);
    for (entity_name, err) in &strategy_failures {
        error!(
            "[finish_rest_integration] Entity '{entity_name}' will NOT sync — failed to build \
             strategy: {err:#}. Integration '{provider_name}' still connects (disclosed \
             degradation)."
        );
    }

    reconcile_sidecar_views(&sidecar, &db_handle, &provider_name).await?;

    // REST exposes no write operations: a read-only provider whose
    // `execute_operation` fails loud, but whose entity readers still back
    // cache reads.
    let operation_provider = McpOperationProvider::read_only(sidecar.clone(), entity_readers);

    // REST has no MCP peer and cannot subscribe.
    let resource_capabilities = ProbedResourceCapabilities::from_server(None);

    // Poll cadence per REST sync entity: per-entity `sync.interval` wins, then
    // the transport-wide `poll_interval`, then the built-in default. REST has no
    // subscription freshness, so every sync entity MUST poll — unbounded
    // staleness must never be silent.
    let default_interval = poll_interval
        .map(|i| i.duration())
        .unwrap_or(DEFAULT_REST_POLL_INTERVAL);
    let poll_entities: Vec<(String, Duration)> = sidecar
        .entities
        .iter()
        .filter(|(_, config)| config.sync.is_some())
        .map(|(name, config)| {
            let every = config
                .sync
                .as_ref()
                .and_then(|s| s.interval)
                .map(|i| i.duration())
                .unwrap_or(default_interval);
            (name.clone(), every)
        })
        .collect();

    let sync_engine = Arc::new(McpSyncEngine::new(
        surface,
        None,
        strategies,
        caches,
        token_store,
        provider_name.clone(),
        sidecar.clone(),
        Vec::new(),
        Some(db_handle),
    ));

    info!(
        "provider '{provider_name}': rest transport connected — poll-only runner over {} sync \
         entit(ies)",
        poll_entities.len()
    );

    Ok(spawn_runner(
        operation_provider,
        sync_engine,
        McpRunningService::inert(),
        resource_capabilities,
        Vec::new(),
        poll_entities,
        None,
        sync_gate,
    ))
}

/// The sync operations the serialized loop drives. Abstracted from
/// [`McpSyncEngine`] so the gate + debounce logic is unit-testable against a
/// counting fake with no live MCP peer.
#[async_trait::async_trait]
pub trait ResyncSink: Send + Sync {
    async fn sync_all(&self) -> anyhow::Result<()>;
    async fn resync_by_uri(&self, uri: &str) -> anyhow::Result<()>;
    async fn sync_entity_by_name(&self, entity: &str) -> anyhow::Result<()>;
}

#[async_trait::async_trait]
impl ResyncSink for McpSyncEngine {
    async fn sync_all(&self) -> anyhow::Result<()> {
        McpSyncEngine::sync_all(self).await
    }
    async fn resync_by_uri(&self, uri: &str) -> anyhow::Result<()> {
        McpSyncEngine::resync_by_uri(self, uri).await
    }
    async fn sync_entity_by_name(&self, entity: &str) -> anyhow::Result<()> {
        McpSyncEngine::sync_entity_by_name(self, entity).await
    }
}

/// Timing knobs for the serialized sync loop. `Default` carries production
/// values; [`SyncLoopTuning::test`] collapses them for fast tests.
#[derive(Debug, Clone, Copy)]
pub struct SyncLoopTuning {
    /// Trailing-edge debounce: a pending re-sync absorbs further signals until
    /// the stream is quiet for this long, then runs once.
    pub debounce: Duration,
    /// Hard coalescing ceiling: under sustained signals that never quiet,
    /// force a drain this long after the first pending signal so re-syncs
    /// can't be starved.
    pub max_coalesce: Duration,
    /// How often to loudly re-warn while a sync is still deferred behind the
    /// boot scan gate — makes a long/stuck deferral visible, never silent.
    pub gate_warn_every: Duration,
    /// Absolute deferral ceiling: if the gate has still not opened after this,
    /// proceed with the sync in DISCLOSED degraded mode (fail-loud: a deferred
    /// sync must eventually run). Sized well above any realistic boot scan.
    pub gate_watchdog: Duration,
}

impl Default for SyncLoopTuning {
    fn default() -> Self {
        Self {
            debounce: Duration::from_secs(2),
            max_coalesce: Duration::from_secs(10),
            gate_warn_every: Duration::from_secs(60),
            gate_watchdog: Duration::from_secs(600),
        }
    }
}

impl SyncLoopTuning {
    /// Near-immediate timings for tests that assert coalescing without waiting
    /// wall-clock seconds.
    pub fn test() -> Self {
        Self {
            debounce: Duration::from_millis(40),
            max_coalesce: Duration::from_millis(200),
            gate_warn_every: Duration::from_millis(50),
            gate_watchdog: Duration::from_secs(5),
        }
    }
}

/// Coalesced sync work accumulated between drains. Deduplicates per resource so
/// N rapid "resource updated" signals for one URI collapse into ONE re-sync.
#[derive(Debug, Default)]
struct PendingSyncWork {
    sync_all: bool,
    uris: HashSet<String>,
    poll_entities: HashSet<String>,
}

impl PendingSyncWork {
    fn absorb(&mut self, event: SyncEvent) {
        match event {
            SyncEvent::SyncAll => self.sync_all = true,
            SyncEvent::NotificationUri(uri) => {
                self.uris.insert(uri);
            }
            SyncEvent::PollTick(entity) => {
                self.poll_entities.insert(entity);
            }
        }
    }

    fn is_empty(&self) -> bool {
        !self.sync_all && self.uris.is_empty() && self.poll_entities.is_empty()
    }

    async fn execute<S: ResyncSink + ?Sized>(self, sync_engine: &S) {
        // A full sync covers every entity, so it subsumes any pending per-URI
        // resyncs and poll ticks that queued alongside it.
        if self.sync_all {
            let span = tracing::info_span!("initial_sync");
            async {
                if let Err(e) = sync_engine.sync_all().await {
                    warn!(error = %e, "initial sync failed");
                }
            }
            .instrument(span)
            .await;
            return;
        }
        for uri in self.uris {
            let span = tracing::info_span!("subscription_resync", %uri);
            async {
                info!("resource updated, re-syncing (coalesced)...");
                if let Err(e) = sync_engine.resync_by_uri(&uri).await {
                    warn!(error = %e, "failed to resync");
                }
            }
            .instrument(span)
            .await;
        }
        for entity in self.poll_entities {
            let span = tracing::info_span!("poll_resync", %entity);
            async {
                if let Err(e) = sync_engine.sync_entity_by_name(&entity).await {
                    warn!(error = %e, "poll resync failed");
                }
            }
            .instrument(span)
            .await;
        }
    }
}

/// Wait for the boot-scan gate to open before any sync runs, keeping a stuck
/// deferral visible and guaranteeing the sync eventually runs.
async fn await_gate(gate: &SyncGate, tuning: &SyncLoopTuning) {
    if gate.state() == holon_core::SyncGateState::Open {
        return;
    }
    info!("sync deferred until org initial scan completes");
    let started = tokio::time::Instant::now();
    let mut warn_tick =
        tokio::time::interval_at(started + tuning.gate_warn_every, tuning.gate_warn_every);
    let watchdog = tokio::time::sleep(tuning.gate_watchdog);
    tokio::pin!(watchdog);
    loop {
        tokio::select! {
            r = gate.wait_open() => {
                match r {
                    Ok(()) => info!("org initial scan complete — sync gate open"),
                    Err(e) => warn!(error = %e, "sync gate closed during teardown — proceeding"),
                }
                return;
            }
            _ = &mut watchdog => {
                error!(
                    waited_s = started.elapsed().as_secs(),
                    "sync gate never opened — org initial scan may be wedged; proceeding with sync \
                     in DISCLOSED degraded mode (may contend with the scan)"
                );
                return;
            }
            _ = warn_tick.tick() => {
                warn!(
                    waited_s = started.elapsed().as_secs(),
                    "sync still deferred — org initial scan in progress"
                );
            }
        }
    }
}

/// Spawn the single consumer that serializes all sync work for an integration:
/// the initial full sync, notification-driven resyncs, and poll ticks.
///
/// Two levers keep provider syncs from starving the boot org scan on the
/// serialized `DatabaseActor`:
/// 1. DEFER — nothing runs until `gate` opens (org initial scan complete);
///    events that arrive during the scan buffer in the channel and coalesce.
/// 2. DEBOUNCE — per-resource trailing-edge coalescing so a storm of "resource
///    updated" notifications collapses into one re-sync per URI.
pub fn spawn_sync_event_loop<S: ResyncSink + 'static>(
    mut receiver: mpsc::UnboundedReceiver<SyncEvent>,
    sync_engine: Arc<S>,
    gate: SyncGate,
    tuning: SyncLoopTuning,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        // Lever 1: hold everything until the boot scan is done. Signals queue
        // in the unbounded channel meanwhile and are drained + coalesced below.
        await_gate(&gate, &tuning).await;

        // Lever 2: trailing-edge + max-wait debounce over the serialized stream.
        let mut pending = PendingSyncWork::default();
        let mut trailing: Option<tokio::time::Instant> = None;
        let mut hard: Option<tokio::time::Instant> = None;

        loop {
            let fire_at = match (trailing, hard) {
                (Some(t), Some(h)) => Some(t.min(h)),
                (Some(t), None) => Some(t),
                (None, Some(h)) => Some(h),
                (None, None) => None,
            };
            tokio::select! {
                maybe_event = receiver.recv() => {
                    match maybe_event {
                        Some(event) => {
                            if pending.is_empty() {
                                hard = Some(tokio::time::Instant::now() + tuning.max_coalesce);
                            }
                            pending.absorb(event);
                            trailing = Some(tokio::time::Instant::now() + tuning.debounce);
                        }
                        None => {
                            if !pending.is_empty() {
                                std::mem::take(&mut pending).execute(sync_engine.as_ref()).await;
                            }
                            break;
                        }
                    }
                }
                _ = async {
                    match fire_at {
                        Some(d) => tokio::time::sleep_until(d).await,
                        None => std::future::pending::<()>().await,
                    }
                }, if fire_at.is_some() => {
                    trailing = None;
                    hard = None;
                    std::mem::take(&mut pending).execute(sync_engine.as_ref()).await;
                }
            }
        }
        info!("[sync_event_loop] Channel closed, stopping");
    })
}

/// EntityFieldReader adapter for a dynamic-entity cache.
struct DynamicEntityFieldReader(Arc<dyn EntityCache<DynamicEntity>>);

impl EntityFieldReader for DynamicEntityFieldReader {
    fn get_fields(
        &self,
        id: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = holon_core::traits::Result<Option<holon_api::StorageEntity>>,
                > + Send
                + '_,
        >,
    > {
        use holon_api::entity::IntoEntity;

        let id = id.to_string();
        Box::pin(async move {
            let entity: Option<DynamicEntity> = self.0.get_by_id(&id).await?;
            Ok(entity.map(|e| e.to_entity().fields.into_iter().collect()))
        })
    }
}

#[cfg(test)]
mod entity_type_registration_tests {
    use holon_api::entity::FieldSchema;
    use holon_profiles::TypeRegistry;

    use super::*;

    fn sidecar_declaring(entity: &str) -> McpSidecar {
        let yaml = format!(
            "entities:\n  {entity}:\n    id_column: id\n    schema:\n      - name: id\n        \
             sql_type: TEXT\n        primary_key: true\n"
        );
        McpSidecar::from_yaml(&yaml).expect("sidecar yaml parses")
    }

    /// The prefix is part of the name, so a prefixed key and another sidecar's
    /// unprefixed key can land on one entity. Registration alone cannot catch
    /// it: it refuses on the (entity, op) pair, and two connectors naming their
    /// tools differently overlap on none.
    #[test]
    fn two_sidecars_resolving_to_one_entity_name_are_refused() {
        let prefixed = McpSidecar::from_yaml(
            "entity_prefix: todoist_\nentities:\n  tasks:\n    short_name: t\n",
        )
        .expect("yaml parses");
        let plain = McpSidecar::from_yaml("entities:\n  todoist_tasks:\n    short_name: t\n")
            .expect("yaml parses");

        let err = assert_no_cross_sidecar_entity_collisions([
            (
                "other-connector",
                prefixed.entity_prefix.as_deref(),
                prefixed.entities.keys().map(String::as_str),
            ),
            (
                "todoist",
                plain.entity_prefix.as_deref(),
                plain.entities.keys().map(String::as_str),
            ),
        ])
        .expect_err("both resolve to 'todoist-tasks'");
        let msg = err.to_string();
        assert!(
            msg.contains("other-connector")
                && msg.contains("todoist")
                && msg.contains("todoist-tasks"),
            "the refusal must name both integrations and the shared entity, got: {msg}"
        );
    }

    /// The set we actually ship must be unambiguous — this is the guard that
    /// makes a future sidecar's prefix choice a failing test rather than a
    /// silent second claimant.
    #[test]
    fn the_bundled_sidecars_claim_distinct_entity_names() {
        let parsed: Vec<(&str, McpSidecar)> = crate::BUNDLED_SIDECARS
            .iter()
            .map(|b| {
                (
                    b.provider,
                    McpSidecar::from_yaml(b.yaml)
                        .unwrap_or_else(|e| panic!("bundled '{}': {e:#}", b.provider)),
                )
            })
            .collect();
        assert_no_cross_sidecar_entity_collisions(parsed.iter().map(|(n, s)| {
            (
                *n,
                s.entity_prefix.as_deref(),
                s.entities.keys().map(String::as_str),
            )
        }))
        .expect("shipped sidecars must claim distinct entity names");
    }

    #[test]
    fn an_entity_colliding_with_a_local_type_is_refused_by_name() {
        let registry = TypeRegistry::new();
        registry
            .register(holon_api::TypeDefinition::new(
                "block",
                vec![FieldSchema {
                    name: "id".to_string(),
                    sql_type: "TEXT".to_string(),
                    primary_key: true,
                    ..Default::default()
                }],
            ))
            .expect("seed the local type");

        let err = register_sidecar_entity_types(&sidecar_declaring("block"), "todoist", &registry)
            .expect_err("a sidecar may not take over a local type's name");
        let msg = err.to_string();
        assert!(
            msg.contains("todoist") && msg.contains("block"),
            "the refusal must name the provider and the entity, got: {msg}"
        );
        assert_eq!(
            registry
                .get("block")
                .expect("local type survives")
                .fields
                .len(),
            1,
            "the refused registration must leave the existing definition untouched"
        );
    }

    #[test]
    fn two_providers_claiming_one_name_is_refused() {
        let registry = TypeRegistry::new();
        let sidecar = sidecar_declaring("tasks");
        register_sidecar_entity_types(&sidecar, "todoist", &registry).expect("first provider wins");

        let err = register_sidecar_entity_types(&sidecar, "asana", &registry)
            .expect_err("the second provider must not silently replace the first");
        assert!(
            err.to_string().contains("todoist"),
            "the refusal must name the owner it collided with, got: {err}"
        );
        assert_eq!(
            registry
                .get("tasks")
                .expect("type survives")
                .declaring_integration(),
            Some("todoist"),
            "the definition still belongs to the provider that registered it first"
        );
    }

    /// A reconnect re-runs registration with the same definition. That is the
    /// one repeat that must not be an error.
    #[test]
    fn the_same_provider_registering_twice_is_a_no_op() {
        let registry = TypeRegistry::new();
        let sidecar = sidecar_declaring("tasks");
        register_sidecar_entity_types(&sidecar, "todoist", &registry).expect("first registration");
        register_sidecar_entity_types(&sidecar, "todoist", &registry)
            .expect("re-registering the same provider's own entity is not an error");
    }
}

#[cfg(test)]
mod integration_resilience_tests {
    //! BugFunnel row 27: an auto-discovered entity whose resource template has
    //! an unexpandable param used to abort the WHOLE integration at
    //! `into_strategy`. These tests pin the two fixes: (1) a parameterized
    //! template is not turned into an unbuildable list sync, and (2) even if a
    //! bad strategy config slips through, it is skipped (not fatal) so the
    //! declared entities survive.

    use super::*;
    use crate::mcp_resource_discovery::is_concrete_uri;

    fn entity(sync: Option<SyncConfig>) -> EntityConfig {
        EntityConfig {
            short_name: None,
            source_name: None,
            id_column: Some("id".into()),
            schema: Vec::new(),
            sync,
            vtable: None,
            profile_variants: Vec::new(),
        }
    }

    fn tool_sync() -> SyncConfig {
        SyncConfig {
            list_tool: Some("list_sessions".into()),
            extract_path: Some("data".into()),
            list_params: HashMap::new(),
            cursor: None,
            list_resource: None,
            uri_params: HashMap::new(),
            interval: None,
            project: HashMap::new(),
        }
    }

    /// The exact unbuildable config auto-discovery produced pre-fix: a
    /// parameterized `list_resource` with no `uri_params` to expand it.
    fn parameterized_resource_sync() -> SyncConfig {
        SyncConfig {
            list_tool: None,
            extract_path: None,
            list_params: HashMap::new(),
            cursor: None,
            list_resource: Some("claude-history://projects/{project_id}/plan".into()),
            uri_params: HashMap::new(),
            interval: None,
            project: HashMap::new(),
        }
    }

    #[test]
    fn parameterized_template_is_not_a_listable_resource() {
        assert!(!is_concrete_uri(
            "claude-history://projects/{project_id}/plan"
        ));
        assert!(is_concrete_uri("claude-history://projects"));
    }

    #[test]
    fn parameterized_resource_sync_fails_into_strategy() {
        // Documents the trigger: without the shape fix, this is the config
        // auto-discovery attached, and it errors when built.
        assert!(parameterized_resource_sync().into_strategy().is_err());
    }

    #[test]
    fn one_unbuildable_entity_does_not_sink_the_declared_ones() {
        let mut entities: HashMap<String, EntityConfig> = HashMap::new();
        entities.insert("session".into(), entity(Some(tool_sync())));
        entities.insert("plan".into(), entity(Some(parameterized_resource_sync())));
        entities.insert("message".into(), entity(None));

        let (strategies, failures) = build_entity_strategies(&entities);

        // The declared, working entity survives.
        assert!(strategies.contains_key("session"));
        // The unbuildable one is reported, not fatal.
        assert_eq!(failures.len(), 1);
        assert_eq!(failures[0].0, "plan");
        // The sync-less entity contributes neither a strategy nor a failure.
        assert!(!strategies.contains_key("message"));
    }

    #[test]
    fn rest_rejects_vtable_entity() {
        // vtable needs an MCP peer to back the FDW cursor — out of scope for
        // the peerless `rest` transport.
        let sidecar: McpSidecar = serde_yaml::from_str(
            "entities:\n  thing:\n    id_column: id\n    vtable:\n      list_resource: \"x://y\"\n      write_through: true\n",
        )
        .expect("sidecar parses");
        let err = reject_rest_out_of_scope(&sidecar, "p").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("vtable"),
            "expected vtable rejection, got: {msg}"
        );
        assert!(
            msg.contains("write_through=true"),
            "should disclose write_through: {msg}"
        );
    }

    #[test]
    fn rest_rejects_list_resource_sync() {
        // REST serves GET *calls*, not MCP resources — `list_resource` sync is
        // a config error naming the wrong mechanism.
        let sidecar: McpSidecar = serde_yaml::from_str(
            "entities:\n  thing:\n    id_column: id\n    sync:\n      list_resource: \"x://y\"\n",
        )
        .expect("sidecar parses");
        let err = reject_rest_out_of_scope(&sidecar, "p").unwrap_err();
        assert!(
            err.to_string().contains("list_resource"),
            "expected list_resource rejection, got: {err}"
        );
    }

    #[test]
    fn rest_accepts_list_tool_sync() {
        // The in-scope shape (GET call via list_tool) passes the guard.
        let sidecar: McpSidecar = serde_yaml::from_str(
            "entities:\n  thing:\n    id_column: id\n    sync:\n      list_tool: list-things\n",
        )
        .expect("sidecar parses");
        reject_rest_out_of_scope(&sidecar, "p").expect("list_tool sync is in scope for rest");
    }
}

#[cfg(test)]
mod sync_loop_gate_debounce_tests {
    //! Boot MCP-sync-vs-scan contention fix. Reproduces the storm: a mock MCP
    //! resource-update flood during a simulated scan window, asserting the two
    //! levers — DEFER (nothing runs before the gate opens) and
    //! DEBOUNCE/COALESCE (N rapid signals per resource collapse into one
    //! re-sync).

    use std::sync::Mutex;
    use std::sync::atomic::AtomicUsize;
    use std::sync::atomic::Ordering::SeqCst;
    use std::time::Duration;

    use super::*;

    #[derive(Default)]
    struct CountingSink {
        sync_all: AtomicUsize,
        resyncs: Mutex<Vec<String>>,
        polls: Mutex<Vec<String>>,
    }

    impl CountingSink {
        fn total(&self) -> usize {
            self.sync_all.load(SeqCst)
                + self.resyncs.lock().unwrap().len()
                + self.polls.lock().unwrap().len()
        }
    }

    #[async_trait::async_trait]
    impl ResyncSink for CountingSink {
        async fn sync_all(&self) -> anyhow::Result<()> {
            self.sync_all.fetch_add(1, SeqCst);
            Ok(())
        }
        async fn resync_by_uri(&self, uri: &str) -> anyhow::Result<()> {
            self.resyncs.lock().unwrap().push(uri.to_string());
            Ok(())
        }
        async fn sync_entity_by_name(&self, entity: &str) -> anyhow::Result<()> {
            self.polls.lock().unwrap().push(entity.to_string());
            Ok(())
        }
    }

    const PROJECTS: &str = "claude-history://projects";
    const TASKS: &str = "claude-history://tasks";
    const SESSIONS: &str = "claude-history://sessions";

    /// (a) Zero sync work runs while the gate is closed (scan in progress);
    /// (b) after the gate opens, the buffered storm collapses to ONE coalesced
    /// full sync (a pending `SyncAll` subsumes the per-URI resyncs).
    #[tokio::test(start_paused = true)]
    async fn nothing_runs_before_scan_complete_then_one_coalesced_sync() {
        let sink = Arc::new(CountingSink::default());
        let gate = SyncGate::new(); // DeferredUntilScan
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = spawn_sync_event_loop(rx, sink.clone(), gate.clone(), SyncLoopTuning::test());

        // Boot enqueues the initial sync, then a resource-update storm arrives
        // during the (still-running) scan.
        tx.send(SyncEvent::SyncAll).unwrap();
        for _ in 0..47 {
            tx.send(SyncEvent::NotificationUri(PROJECTS.to_string()))
                .unwrap();
            tx.send(SyncEvent::NotificationUri(TASKS.to_string()))
                .unwrap();
        }

        // Advance well past the debounce window while the gate is CLOSED.
        tokio::time::sleep(Duration::from_millis(500)).await;
        assert_eq!(
            sink.total(),
            0,
            "no sync may run before the org scan completes (gate closed)"
        );

        // Scan completes → gate opens.
        gate.open();
        tokio::time::sleep(Duration::from_millis(500)).await;

        assert_eq!(
            sink.sync_all.load(SeqCst),
            1,
            "the buffered storm collapses to exactly one full sync"
        );
        assert_eq!(
            sink.resyncs.lock().unwrap().len(),
            0,
            "per-URI resyncs are subsumed by the coalesced full sync"
        );

        drop(tx);
        let _ = handle.await;
    }

    /// A storm of rapid notifications for the SAME resource collapses into one
    /// re-sync; distinct resources each get exactly one.
    #[tokio::test(start_paused = true)]
    async fn debounce_collapses_per_resource() {
        let sink = Arc::new(CountingSink::default());
        let (tx, rx) = mpsc::unbounded_channel();
        let handle =
            spawn_sync_event_loop(rx, sink.clone(), SyncGate::opened(), SyncLoopTuning::test());

        for _ in 0..47 {
            tx.send(SyncEvent::NotificationUri(PROJECTS.to_string()))
                .unwrap();
        }
        for _ in 0..16 {
            tx.send(SyncEvent::NotificationUri(TASKS.to_string()))
                .unwrap();
            tx.send(SyncEvent::NotificationUri(SESSIONS.to_string()))
                .unwrap();
        }

        tokio::time::sleep(Duration::from_millis(500)).await;

        let mut got = sink.resyncs.lock().unwrap().clone();
        got.sort();
        assert_eq!(
            got,
            vec![
                PROJECTS.to_string(),
                SESSIONS.to_string(),
                TASKS.to_string()
            ],
            "79 signals across 3 resources collapse to one re-sync each"
        );

        drop(tx);
        let _ = handle.await;
    }

    /// The gate's watchdog guarantees a deferred sync eventually runs even if
    /// the gate never opens (fail-loud: disclosed degraded mode, never a silent
    /// never-sync).
    #[tokio::test(start_paused = true)]
    async fn watchdog_runs_sync_if_gate_never_opens() {
        let sink = Arc::new(CountingSink::default());
        let (tx, rx) = mpsc::unbounded_channel();
        // Gate stays DeferredUntilScan forever (no open() call).
        let handle =
            spawn_sync_event_loop(rx, sink.clone(), SyncGate::new(), SyncLoopTuning::test());
        tx.send(SyncEvent::NotificationUri(PROJECTS.to_string()))
            .unwrap();

        // Past the test watchdog (5s) + debounce.
        tokio::time::sleep(Duration::from_secs(6)).await;
        assert_eq!(
            sink.resyncs.lock().unwrap().as_slice(),
            &[PROJECTS.to_string()],
            "watchdog proceeds so the deferred sync still runs"
        );

        drop(tx);
        let _ = handle.await;
    }
}

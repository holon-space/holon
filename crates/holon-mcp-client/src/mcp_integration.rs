use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Context;
use holon_api::DynamicEntity;
use holon_core::CacheFactory;
use holon_core::EntityCache;
use holon_core::SyncTokenStore;
use holon_turso::turso::DbHandle;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::Instrument;
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
use crate::mcp_resource_discovery::parse_resource_template_meta;
use crate::mcp_sidecar::EntityConfig;
use crate::mcp_sidecar::McpSidecar;
use crate::mcp_sidecar::SyncConfig;
use crate::mcp_sync_engine::McpSyncEngine;
use crate::mcp_sync_strategy::SyncStrategy;
use crate::sync_freshness::ProbedResourceCapabilities;

/// Transport configuration for connecting to an MCP server.
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

    /// Register all entity types from the sidecar config into the TypeRegistry.
    /// Called by frontends after building the integration so GQL graph includes
    /// MCP entities.
    pub fn register_entity_types(&self, type_registry: &holon_profiles::TypeRegistry) {
        let sidecar = self.sync_engine.sidecar();
        for (entity_name, entity_config) in &sidecar.entities {
            let table_name = sidecar.prefixed_name(entity_name).table_name();
            if let Some(td) = entity_config.to_type_definition(&table_name)
                && let Err(e) = type_registry.register(td)
            {
                tracing::warn!(
                    "[McpIntegration] Failed to register type '{}': {e}",
                    table_name
                );
            }
        }
    }
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
            )
            .await?;
            Ok(McpConnectionResult::Connected(integration))
        }
    }
}

/// Attempt OAuth connection: use stored tokens if available, otherwise return
/// NeedsAuth.
async fn build_oauth_integration(
    uri: String,
    credential_store: Arc<TursoCredentialStore>,
    sidecar: McpSidecar,
    db_handle: DbHandle,
    cache_factory: Arc<dyn CacheFactory>,
    token_store: Arc<dyn SyncTokenStore>,
    provider_name: String,
    pending_flows: &PendingOAuthFlows,
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
            },
        )
        .await;

    Ok(McpConnectionResult::NeedsAuth {
        auth_url,
        provider_name,
    })
}

/// Common integration finalization: build caches, discover resources, build
/// strategies, subscribe.
async fn finish_integration(
    peer: rmcp::service::Peer<rmcp::RoleClient>,
    service: McpRunningService,
    mut sidecar: McpSidecar,
    db_handle: DbHandle,
    cache_factory: Arc<dyn CacheFactory>,
    token_store: Arc<dyn SyncTokenStore>,
    provider_name: String,
    receiver: ResourceUpdateReceiver,
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

            info!(
                "[finish_integration] Auto-discovered entity '{}' from resource template '{}'",
                meta.entity_name, meta.uri_template
            );

            sidecar.entities.insert(
                meta.entity_name.clone(),
                EntityConfig {
                    short_name: Some(short_name),
                    source_name: None,
                    id_column: Some(id_column),
                    schema: meta.fields,
                    sync: Some(SyncConfig {
                        list_tool: None,
                        extract_path: None,
                        list_params: HashMap::new(),
                        cursor: None,
                        list_resource: Some(meta.uri_template),
                        uri_params: HashMap::new(),
                        interval: None,
                    }),
                    vtable: None,
                    profile_variants: Vec::new(),
                },
            );
        }
    }

    // Build caches and strategies.
    // Table names and ID schemes use prefixed names (e.g. "cc_session"),
    // but internal keys use original entity names (e.g. "session").
    let mut caches: HashMap<String, Arc<dyn EntityCache<DynamicEntity>>> = HashMap::new();
    let mut entity_readers: HashMap<String, Arc<dyn EntityFieldReader>> = HashMap::new();
    let mut strategies: HashMap<String, Box<dyn SyncStrategy>> = HashMap::new();

    for (entity_name, entity_config) in &sidecar.entities {
        let entity = sidecar.prefixed_name(entity_name);
        let table_name = entity.table_name();
        if let Some(td) = entity_config.to_type_definition(&table_name) {
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

        if let Some(ref sync_config) = entity_config.sync {
            let strategy = sync_config.into_strategy().with_context(|| {
                format!("[finish_integration] Failed to build strategy for '{entity_name}'")
            })?;
            strategies.insert(entity_name.clone(), strategy);
        }
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

    // Sidecar-declared derived views (generic): the cache tables they select
    // from were just created by the CacheFactory above. A view that fails DDL
    // is a hard, loud config error naming the view and the provider — never
    // skip-and-continue (parse, don't validate, at connect).
    for view in &sidecar.views {
        let view_name = sidecar.prefixed_name(&view.name).table_name();
        holon_turso::matview_manager::reconcile_named_view(&db_handle, &view_name, &view.sql)
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
        peer,
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

    // One serialized consumer per integration: initial sync, notification
    // resyncs, and poll ticks all flow through the same channel, so per-entity
    // sync work never overlaps.
    let (sync_event_tx, sync_event_rx) = mpsc::unbounded_channel::<SyncEvent>();
    let sync_event_task = spawn_sync_event_loop(sync_event_rx, sync_engine.clone());

    let mut background_tasks = Vec::new();

    // Notification forwarder: resource-updated URIs -> serialized consumer.
    {
        let tx = sync_event_tx.clone();
        let mut receiver = receiver;
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

    Ok(McpIntegration {
        operation_provider,
        sync_engine,
        service,
        sync_event_task,
        background_tasks,
        resource_capabilities,
        fdw_backed_tables,
        sync_event_tx,
    })
}

/// Spawn the single consumer that serializes all sync work for an integration:
/// the initial full sync, notification-driven resyncs, and poll ticks.
pub fn spawn_sync_event_loop(
    mut receiver: mpsc::UnboundedReceiver<SyncEvent>,
    sync_engine: Arc<McpSyncEngine>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(event) = receiver.recv().await {
            match event {
                SyncEvent::SyncAll => {
                    let span = tracing::info_span!("initial_sync");
                    async {
                        if let Err(e) = sync_engine.sync_all().await {
                            warn!(error = %e, "initial sync failed");
                        }
                    }
                    .instrument(span)
                    .await;
                }
                SyncEvent::NotificationUri(uri) => {
                    let span = tracing::info_span!("subscription_resync", %uri);
                    async {
                        info!("resource updated, re-syncing...");
                        if let Err(e) = sync_engine.resync_by_uri(&uri).await {
                            warn!(error = %e, "failed to resync");
                        }
                    }
                    .instrument(span)
                    .await;
                }
                SyncEvent::PollTick(entity) => {
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

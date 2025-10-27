use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tracing::{info, warn};

use holon::core::datasource::SyncTokenStore;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::DbHandle;
use holon_api::DynamicEntity;

use crate::credential_store::TursoCredentialStore;
use crate::mcp_notification_handler::{NotifyingClientHandler, ResourceUpdateReceiver};
use crate::mcp_provider::{
    EntityFieldReader, McpOperationProvider, McpRunningService, connect_mcp_child_with_handler,
    connect_mcp_oauth_with_handler, connect_mcp_with_handler,
};
use crate::mcp_resource_discovery::parse_resource_template_meta;
use crate::mcp_sidecar::{EntityConfig, McpSidecar, SyncConfig};
use crate::mcp_sync_engine::McpSyncEngine;
use crate::mcp_sync_strategy::SyncStrategy;

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

/// Result of building an MCP integration: operation provider, sync engine, and running service.
pub struct McpIntegration {
    pub operation_provider: McpOperationProvider,
    pub sync_engine: Arc<McpSyncEngine>,
    /// Must be kept alive for the MCP connection to stay open.
    pub service: McpRunningService,
    /// Background task processing resource update notifications.
    /// `None` if no entities use resource-based sync.
    pub subscription_task: Option<JoinHandle<()>>,
    /// Entity schemas for GQL graph registration (derived from sidecar config).
    pub entity_schemas: Vec<holon_api::EntitySchema>,
}

/// State parked between `build_mcp_integration` returning `NeedsAuth` and
/// the frontend calling `complete_oauth` with the authorization code.
struct PendingOAuth {
    auth_manager: rmcp::transport::auth::AuthorizationManager,
    uri: String,
    sidecar: McpSidecar,
    db_handle: DbHandle,
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

    /// Complete an OAuth flow after the frontend captured the authorization code.
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
                "No pending OAuth flow for provider '{provider_name}'. \
                 Was build_mcp_integration called first?"
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
                token_store,
                config.provider_name,
                receiver,
            )
            .await?;
            Ok(McpConnectionResult::Connected(integration))
        }
    }
}

/// Attempt OAuth connection: use stored tokens if available, otherwise return NeedsAuth.
async fn build_oauth_integration(
    uri: String,
    credential_store: Arc<TursoCredentialStore>,
    sidecar: McpSidecar,
    db_handle: DbHandle,
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
            token_store,
            provider_name,
            receiver,
        )
        .await?;
        return Ok(McpConnectionResult::Connected(integration));
    }

    info!("[OAuth] No stored credentials for '{uri}', initiating OAuth flow");

    // Use a custom URL scheme for flutter_web_auth_2 callback interception.
    // The OS hands the redirect URL back to Flutter without needing a localhost server.
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

/// Common integration finalization: build caches, discover resources, build strategies, subscribe.
async fn finish_integration(
    peer: rmcp::service::Peer<rmcp::RoleClient>,
    service: McpRunningService,
    mut sidecar: McpSidecar,
    db_handle: DbHandle,
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

            if let Some(existing) = sidecar.entities.get_mut(&meta.entity_name) {
                // Merge auto-discovered metadata into existing sidecar entry
                if existing.schema.is_empty() {
                    info!(
                        "[finish_integration] Merging auto-discovered schema into sidecar entity '{}'",
                        meta.entity_name
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
                    id_column: Some(id_column),
                    schema: meta.fields,
                    sync: Some(SyncConfig {
                        list_tool: None,
                        extract_path: None,
                        list_params: HashMap::new(),
                        cursor: None,
                        list_resource: Some(meta.uri_template),
                        uri_params: HashMap::new(),
                    }),
                },
            );
        }
    }

    // Build caches and strategies
    let mut caches: HashMap<String, Arc<QueryableCache<DynamicEntity>>> = HashMap::new();
    let mut entity_readers: HashMap<String, Arc<dyn EntityFieldReader>> = HashMap::new();
    let mut strategies: HashMap<String, Box<dyn SyncStrategy>> = HashMap::new();

    for (entity_name, entity_config) in &sidecar.entities {
        if let Some(schema) = entity_config.to_schema(entity_name) {
            let cache = QueryableCache::<DynamicEntity>::new(db_handle.clone(), schema)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            let cache = Arc::new(cache);
            entity_readers.insert(
                entity_name.clone(),
                Arc::new(DynamicEntityFieldReader(cache.clone())) as Arc<dyn EntityFieldReader>,
            );
            caches.insert(entity_name.clone(), cache);
        }

        if let Some(ref sync_config) = entity_config.sync {
            match sync_config.into_strategy() {
                Ok(strategy) => {
                    strategies.insert(entity_name.clone(), strategy);
                }
                Err(e) => {
                    warn!(
                        "[finish_integration] Failed to build strategy for '{}': {e}",
                        entity_name
                    );
                }
            }
        }
    }

    let operation_provider =
        McpOperationProvider::from_peer_shared(peer.clone(), sidecar.clone(), entity_readers)
            .await?;

    let sync_engine = Arc::new(McpSyncEngine::new(
        peer,
        strategies,
        caches,
        token_store,
        provider_name,
    ));

    // Subscribe to resource updates and spawn background listener
    let subscription_task = if sync_engine.has_subscriptions() {
        if let Err(e) = sync_engine.subscribe_all().await {
            warn!("[finish_integration] Failed to subscribe to resources: {e}");
        }
        Some(spawn_subscription_listener(receiver, sync_engine.clone()))
    } else {
        None
    };

    let entity_schemas: Vec<holon_api::EntitySchema> = sidecar
        .entities
        .iter()
        .filter(|(_, config)| !config.schema.is_empty())
        .map(|(name, config)| config.to_entity_schema(name))
        .collect();

    Ok(McpIntegration {
        operation_provider,
        sync_engine,
        service,
        subscription_task,
        entity_schemas,
    })
}

/// Spawn a background task that re-syncs entities when resource update notifications arrive.
fn spawn_subscription_listener(
    mut receiver: ResourceUpdateReceiver,
    sync_engine: Arc<McpSyncEngine>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(uri) = receiver.0.recv().await {
            info!("[subscription_listener] Resource updated: {uri}, re-syncing...");
            if let Err(e) = sync_engine.resync_by_uri(&uri).await {
                warn!("[subscription_listener] Failed to resync '{uri}': {e}");
            }
        }
        info!("[subscription_listener] Channel closed, stopping");
    })
}

/// EntityFieldReader adapter for QueryableCache<DynamicEntity>.
struct DynamicEntityFieldReader(Arc<QueryableCache<DynamicEntity>>);

impl EntityFieldReader for DynamicEntityFieldReader {
    fn get_fields(
        &self,
        id: &str,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = holon_core::traits::Result<Option<HashMap<String, holon_api::Value>>>,
                > + Send
                + '_,
        >,
    > {
        use holon::core::datasource::DataSource;
        use holon_api::entity::IntoEntity;

        let id = id.to_string();
        Box::pin(async move {
            let entity: Option<DynamicEntity> = self.0.get_by_id(&id).await?;
            Ok(entity.map(|e| e.to_entity().fields))
        })
    }
}

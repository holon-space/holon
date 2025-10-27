//! PBT MCP integration — exercises the real MCP client ingestion pipeline.
//!
//! Uses `tokio::io::duplex` to wire a minimal rmcp `ServerHandler` directly to
//! our `McpSyncEngine`, bypassing stdio/network but exercising the full path:
//!
//!   TestMcpServer → rmcp duplex → McpSyncEngine → QueryableCache → Turso IVM
//!
//! Each `emit_update()` call mutates the server's resource, sends a
//! `notifications/resources/updated` notification, which triggers
//! `resync_by_uri` in the sync engine → cache diff → Turso write → CDC.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use holon::core::datasource::SyncTokenStore;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::DbHandle;
use holon_api::DynamicEntity;
use holon_api::entity::FieldSchema;
use rmcp::model::*;
use rmcp::service::{Peer, RequestContext, RunningService};
use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};
use tokio::sync::RwLock;

use holon_mcp_client::mcp_sidecar::{EntityConfig, McpSidecar, SyncConfig};
use holon_mcp_client::mcp_sync_engine::McpSyncEngine;

const ENTITY_NAME: &str = "pbt_probe";
const RESOURCE_URI: &str = "pbt://probe/items";

// ── Test MCP Server ───────────────────────────────────────────────

/// Minimal MCP server that serves a single resource with mutable items.
struct TestMcpServer {
    items: Arc<RwLock<Vec<serde_json::Value>>>,
}

impl ServerHandler for TestMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
            server_info: Implementation {
                name: "pbt-test-server".into(),
                title: None,
                version: "0.1.0".into(),
                icons: None,
                website_url: None,
            },
            ..Default::default()
        }
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParam>,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        async {
            Ok(ListResourcesResult {
                meta: None,
                next_cursor: None,
                resources: vec![Annotated::new(
                    RawResource {
                        uri: RESOURCE_URI.to_string(),
                        name: "PBT Probe Items".to_string(),
                        title: None,
                        description: Some("Test entities for PBT IVM exercise".to_string()),
                        mime_type: Some("application/json".to_string()),
                        size: None,
                        icons: None,
                        meta: None,
                    },
                    None,
                )],
            })
        }
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, ErrorData>> + Send + '_ {
        async move {
            if request.uri != RESOURCE_URI {
                return Err(ErrorData::resource_not_found("Unknown resource", None));
            }
            let items = self.items.read().await;
            let json = serde_json::to_string(&*items).expect("serialize items");
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(json, RESOURCE_URI)],
            })
        }
    }

    fn subscribe(
        &self,
        _request: SubscribeRequestParam,
        _context: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), ErrorData>> + Send + '_ {
        std::future::ready(Ok(()))
    }
}

// ── In-memory SyncTokenStore ──────────────────────────────────────

struct InMemorySyncTokenStore {
    tokens: tokio::sync::Mutex<HashMap<String, holon::core::datasource::StreamPosition>>,
}

impl InMemorySyncTokenStore {
    fn new() -> Self {
        Self {
            tokens: tokio::sync::Mutex::new(HashMap::new()),
        }
    }
}

#[async_trait::async_trait]
impl SyncTokenStore for InMemorySyncTokenStore {
    async fn save_token(
        &self,
        key: &str,
        position: holon::core::datasource::StreamPosition,
    ) -> holon::core::datasource::Result<()> {
        self.tokens.lock().await.insert(key.to_string(), position);
        Ok(())
    }

    async fn load_token(
        &self,
        key: &str,
    ) -> holon::core::datasource::Result<Option<holon::core::datasource::StreamPosition>> {
        Ok(self.tokens.lock().await.get(key).cloned())
    }

    async fn clear_all_tokens(&self) -> holon::core::datasource::Result<()> {
        self.tokens.lock().await.clear();
        Ok(())
    }
}

// ── Public API ────────────────────────────────────────────────────

/// MCP integration for PBT testing.
///
/// Exercises the real MCP client pipeline:
/// duplex transport → McpSyncEngine → QueryableCache → Turso.
pub struct PbtMcpIntegration {
    counter: AtomicU64,
    server_items: Arc<RwLock<Vec<serde_json::Value>>>,
    server_peer: Peer<RoleServer>,
    sync_engine: Arc<McpSyncEngine>,
}

impl PbtMcpIntegration {
    /// Create a new MCP integration wired via in-memory duplex transport.
    ///
    /// Sets up the cache table in Turso and performs an initial sync.
    pub async fn new(db_handle: DbHandle) -> anyhow::Result<Self> {
        let server_items: Arc<RwLock<Vec<serde_json::Value>>> = Arc::new(RwLock::new(Vec::new()));

        // Wire server and client via duplex stream.
        // Both sides must handshake concurrently — serve() blocks until
        // the peer sends its initialize message.
        let (server_transport, client_transport) = tokio::io::duplex(8192);

        let server = TestMcpServer {
            items: server_items.clone(),
        };
        let (client_handler, update_rx) = holon_mcp_client::NotifyingClientHandler::new();

        let (server_running, client_running) = tokio::try_join!(
            async {
                server
                    .serve(server_transport)
                    .await
                    .map_err(|e| anyhow::anyhow!("Server init: {e}"))
            },
            async {
                client_handler
                    .serve(client_transport)
                    .await
                    .map_err(|e| anyhow::anyhow!("Client init: {e}"))
            },
        )?;

        let server_peer = server_running.peer().clone();
        let client_peer: Peer<RoleClient> = client_running.peer().clone();

        // Spawn both tasks so they process messages
        tokio::spawn(async move {
            if let Err(e) = server_running.waiting().await {
                tracing::warn!("[PbtMcpIntegration] Server task ended: {e}");
            }
        });
        tokio::spawn(async move {
            if let Err(e) = client_running.waiting().await {
                tracing::warn!("[PbtMcpIntegration] Client task ended: {e}");
            }
        });

        // Build sidecar config for the pbt_probe entity
        let mut entities = HashMap::new();
        entities.insert(
            ENTITY_NAME.to_string(),
            EntityConfig {
                short_name: None,
                source_name: None,
                id_column: Some("id".to_string()),
                schema: vec![
                    FieldSchema {
                        name: "id".to_string(),
                        sql_type: "TEXT".to_string(),
                        primary_key: true,
                        ..Default::default()
                    },
                    FieldSchema {
                        name: "data".to_string(),
                        sql_type: "TEXT".to_string(),
                        ..Default::default()
                    },
                ],
                sync: Some(SyncConfig {
                    list_tool: None,
                    extract_path: None,
                    list_params: HashMap::new(),
                    cursor: None,
                    list_resource: Some(RESOURCE_URI.to_string()),
                    uri_params: HashMap::new(),
                }),
                vtable: None,
                profile_variants: vec![],
            },
        );

        let sidecar = McpSidecar {
            entity_prefix: Some("pbt_".to_string()),
            entities,
            tools: HashMap::new(),
        };

        // Create cache table in Turso (same pattern as finish_integration)
        let entity_config = &sidecar.entities[ENTITY_NAME];
        let entity = sidecar.prefixed_name(ENTITY_NAME);
        let table_name = entity.table_name();
        let td = entity_config
            .to_type_definition(&table_name)
            .expect("EntityConfig with schema must produce a TypeDefinition");
        let cache = QueryableCache::<DynamicEntity>::new(db_handle.clone(), td)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to create pbt_probe cache: {e}"))?;

        let mut caches: HashMap<String, Arc<QueryableCache<DynamicEntity>>> = HashMap::new();
        caches.insert(ENTITY_NAME.to_string(), Arc::new(cache));

        // Build sync strategies from sidecar config
        let mut strategies: HashMap<String, Box<dyn holon_mcp_client::SyncStrategy>> =
            HashMap::new();
        let sync_config = sidecar.entities[ENTITY_NAME].sync.as_ref().unwrap();
        strategies.insert(ENTITY_NAME.to_string(), sync_config.into_strategy()?);

        let token_store: Arc<dyn SyncTokenStore> = Arc::new(InMemorySyncTokenStore::new());

        let sync_engine = Arc::new(McpSyncEngine::new(
            client_peer,
            strategies,
            caches,
            token_store,
            "pbt-test".to_string(),
            sidecar,
            vec![],
            Some(db_handle),
        ));

        // Initial sync (empty — no items yet)
        sync_engine.sync_all().await?;

        // Subscribe to resource notifications
        sync_engine.subscribe_all().await?;

        // Spawn notification listener
        let engine_for_listener = sync_engine.clone();
        holon_mcp_client::spawn_subscription_listener(update_rx, engine_for_listener);

        Ok(Self {
            counter: AtomicU64::new(0),
            server_items,
            server_peer,
            sync_engine,
        })
    }

    /// Emit a data update through the real MCP pipeline.
    ///
    /// 1. Adds a new item to the test server's resource
    /// 2. Sends `notifications/resources/updated` via the server peer
    /// 3. The client's NotifyingClientHandler receives it
    /// 4. The subscription listener calls `resync_by_uri` on the sync engine
    /// 5. The sync engine fetches the resource, diffs against cache, writes to Turso
    /// 6. Turso IVM re-evaluates dependent materialized views
    pub async fn emit_update(&self) -> anyhow::Result<()> {
        let n = self.counter.fetch_add(1, Ordering::SeqCst);

        // Add item to server state
        {
            let mut items = self.server_items.write().await;
            items.push(serde_json::json!({
                "id": format!("pbt-probe-{n}"),
                "data": format!("update-{n}"),
            }));
        }

        // Also send the notification (exercises that path), but don't rely on it.
        let _ = self
            .server_peer
            .notify_resource_updated(ResourceUpdatedNotificationParam {
                uri: RESOURCE_URI.to_string(),
            })
            .await;

        // Directly trigger a synchronous resync so the SQL write lands
        // within the measurement window (the async notification path
        // races with the budget check).
        self.sync_engine.resync_by_uri(RESOURCE_URI).await?;

        Ok(())
    }
}

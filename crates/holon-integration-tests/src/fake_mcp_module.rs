//! Fake external MCP provider wired into the FrontendSession DI.
//!
//! Replaces the old Todoist fake as the test suite's "external provider that
//! does concurrent DDL + sync during startup" stressor. It drives the *real*
//! MCP client pipeline over an in-memory `tokio::io::duplex` transport:
//!
//!   in-memory ServerHandler → rmcp duplex → McpSyncEngine → QueryableCache → Turso
//!
//! Registered via [`register_fake_mcp`] from a test's pre-build DI closure
//! (see `test_environment.rs`). Building the integration creates the cache
//! table and kicks off an initial sync concurrently with the rest of startup —
//! the property the PBT harness and `turso_ivm_index_bug` rely on.

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::{Injector, Provider, Shared};
use holon::core::queryable_cache::QueryableCache;
use holon::storage::DbHandle;
use holon_api::DynamicEntity;
use holon_api::entity::FieldSchema;
use holon_core::{OperationProvider, SyncTokenStore, SyncableProvider};
use holon_mcp_client::mcp_sidecar::{EntityConfig, McpSidecar, SyncConfig};
use holon_mcp_client::mcp_sync_engine::McpSyncEngine;
use rmcp::model::*;
use rmcp::service::{Peer, RequestContext};
use rmcp::{RoleClient, RoleServer, ServerHandler, ServiceExt};
use tokio::sync::RwLock;

const ENTITY_NAME: &str = "fake_probe";
const RESOURCE_URI: &str = "fake://probe/items";

// ── In-memory MCP server ──────────────────────────────────────────

/// Minimal MCP server serving a single JSON resource of items.
struct FakeMcpServer {
    items: Arc<RwLock<Vec<serde_json::Value>>>,
}

impl ServerHandler for FakeMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            capabilities: ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
            server_info: Implementation {
                name: "fake-mcp-server".into(),
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
        _: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        async {
            Ok(ListResourcesResult {
                meta: None,
                next_cursor: None,
                resources: vec![Annotated::new(
                    RawResource {
                        uri: RESOURCE_URI.to_string(),
                        name: "Fake Probe Items".to_string(),
                        title: None,
                        description: Some("Fake external entities for test stress".to_string()),
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
        _: RequestContext<RoleServer>,
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
        _: SubscribeRequestParam,
        _: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), ErrorData>> + Send + '_ {
        std::future::ready(Ok(()))
    }
}

// ── In-memory SyncTokenStore ──────────────────────────────────────

struct InMemorySyncTokenStore {
    tokens: tokio::sync::Mutex<HashMap<String, holon_api::StreamPosition>>,
}

#[async_trait::async_trait]
impl SyncTokenStore for InMemorySyncTokenStore {
    async fn save_token(
        &self,
        key: &str,
        position: holon_api::StreamPosition,
    ) -> holon_core::Result<()> {
        self.tokens.lock().await.insert(key.to_string(), position);
        Ok(())
    }
    async fn load_token(&self, key: &str) -> holon_core::Result<Option<holon_api::StreamPosition>> {
        Ok(self.tokens.lock().await.get(key).cloned())
    }
    async fn clear_all_tokens(&self) -> holon_core::Result<()> {
        self.tokens.lock().await.clear();
        Ok(())
    }
}

// ── DI handle ─────────────────────────────────────────────────────

/// Keeps the in-memory MCP pipeline alive for the lifetime of the session.
pub struct FakeMcpHandle {
    sync_engine: Arc<McpSyncEngine>,
    _server_peer: Peer<RoleServer>,
    _items: Arc<RwLock<Vec<serde_json::Value>>>,
}

/// No-op operation provider; exists only to (a) force the handle to build at
/// startup when the dispatcher resolves the provider set, and (b) keep the
/// pipeline alive. The fake serves no dispatched operations.
struct FakeOperationProvider {
    _handle: Arc<FakeMcpHandle>,
}

#[async_trait::async_trait]
impl OperationProvider for FakeOperationProvider {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        vec![]
    }
    async fn execute_operation(
        &self,
        _: &holon_api::EntityName,
        _: &str,
        _: holon_core::storage::types::StorageEntity,
    ) -> holon::core::traits::Result<holon_core::OperationResult> {
        Err("fake MCP provider serves no operations".into())
    }
}

async fn build_handle(db_handle: DbHandle) -> anyhow::Result<FakeMcpHandle> {
    let server_items: Arc<RwLock<Vec<serde_json::Value>>> = Arc::new(RwLock::new(Vec::new()));

    let (server_transport, client_transport) = tokio::io::duplex(8192);
    let server = FakeMcpServer {
        items: server_items.clone(),
    };
    let (client_handler, update_rx) = holon_mcp_client::NotifyingClientHandler::new();

    let (server_running, client_running) = tokio::try_join!(
        async {
            server
                .serve(server_transport)
                .await
                .map_err(|e| anyhow::anyhow!("Fake MCP server init: {e}"))
        },
        async {
            client_handler
                .serve(client_transport)
                .await
                .map_err(|e| anyhow::anyhow!("Fake MCP client init: {e}"))
        },
    )?;

    let server_peer = server_running.peer().clone();
    let client_peer: Peer<RoleClient> = client_running.peer().clone();

    tokio::spawn(async move {
        if let Err(e) = server_running.waiting().await {
            tracing::warn!("[FakeMcp] server task ended: {e}");
        }
    });
    tokio::spawn(async move {
        if let Err(e) = client_running.waiting().await {
            tracing::warn!("[FakeMcp] client task ended: {e}");
        }
    });

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
                interval: None,
            }),
            vtable: None,
            profile_variants: vec![],
        },
    );

    let sidecar = McpSidecar {
        entity_prefix: Some("fake_".to_string()),
        entities,
        tools: HashMap::new(),
        views: vec![],
    };

    let entity_config = &sidecar.entities[ENTITY_NAME];
    let entity = sidecar.prefixed_name(ENTITY_NAME);
    let table_name = entity.table_name();
    let td = entity_config
        .to_type_definition(&table_name)
        .expect("EntityConfig with schema must produce a TypeDefinition");
    let cache = QueryableCache::<DynamicEntity>::new(db_handle.clone(), td)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to create fake_probe cache: {e}"))?;

    let mut caches: HashMap<String, Arc<dyn holon_core::EntityCache<DynamicEntity>>> =
        HashMap::new();
    caches.insert(ENTITY_NAME.to_string(), Arc::new(cache));

    let mut strategies: HashMap<String, Box<dyn holon_mcp_client::SyncStrategy>> = HashMap::new();
    let sync_config = sidecar.entities[ENTITY_NAME].sync.as_ref().unwrap();
    strategies.insert(ENTITY_NAME.to_string(), sync_config.into_strategy()?);

    let token_store: Arc<dyn SyncTokenStore> = Arc::new(InMemorySyncTokenStore {
        tokens: tokio::sync::Mutex::new(HashMap::new()),
    });

    let sync_engine = Arc::new(McpSyncEngine::new(
        client_peer,
        strategies,
        caches,
        token_store,
        "fake-mcp".to_string(),
        sidecar,
        vec![],
        Some(db_handle),
    ));

    sync_engine.sync_all().await?;
    sync_engine.subscribe_all().await?;
    let (sync_event_tx, sync_event_rx) = tokio::sync::mpsc::unbounded_channel();
    holon_mcp_client::spawn_sync_event_loop(sync_event_rx, sync_engine.clone());
    tokio::spawn(async move {
        let mut update_rx = update_rx;
        while let Some(uri) = update_rx.0.recv().await {
            if sync_event_tx
                .send(holon_mcp_client::SyncEvent::NotificationUri(uri))
                .is_err()
            {
                break;
            }
        }
    });

    Ok(FakeMcpHandle {
        sync_engine,
        _server_peer: server_peer,
        _items: server_items,
    })
}

/// Register the fake external MCP provider into `injector`.
///
/// Call from a test's pre-build DI closure. Mirrors `McpIntegrationsModule`:
/// a singleton handle builds the pipeline once (table + initial sync), and the
/// SyncableProvider + OperationProvider set entries resolve from it. Building
/// happens at startup, concurrently with the rest of session bring-up.
pub fn register_fake_mcp(injector: &Injector) {
    injector.provide::<FakeMcpHandle>(Provider::root_async(|resolver| async move {
        let db_handle = resolver
            .resolve_async::<dyn holon::di::DbHandleProvider>()
            .await
            .handle();
        Shared::new(
            build_handle(db_handle)
                .await
                .expect("[FakeMcp] failed to build in-memory MCP integration"),
        )
    }));

    injector.provide_into_set::<dyn SyncableProvider>(Provider::root_async(
        |resolver| async move {
            let handle = resolver.resolve_async::<FakeMcpHandle>().await;
            handle.sync_engine.clone() as Arc<dyn SyncableProvider>
        },
    ));

    injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
        |resolver| async move {
            let handle = resolver.resolve_async::<FakeMcpHandle>().await;
            Arc::new(FakeOperationProvider {
                _handle: handle.clone(),
            }) as Arc<dyn OperationProvider>
        },
    ));
}

//! Fake external MCP provider wired into the FrontendSession DI.
//!
//! Replaces the old Todoist fake as the test suite's "external provider that
//! does concurrent DDL + sync during startup" stressor. It drives the *real*
//! MCP client pipeline over an in-memory `tokio::io::duplex` transport:
//!
//!   in-memory ServerHandler → rmcp duplex → McpSyncEngine → QueryableCache →
//! Turso
//!
//! Registered via [`register_fake_mcp`] from a test's pre-build DI closure
//! (see `test_environment.rs`). Building the integration creates the cache
//! table and kicks off an initial sync concurrently with the rest of startup —
//! the property the PBT harness and `turso_ivm_index_bug` rely on.

use std::collections::HashMap;
use std::sync::Arc;

use fluxdi::Injector;
use fluxdi::Provider;
use fluxdi::Shared;
use holon::core::queryable_cache::QueryableCache;
use holon::storage::DbHandle;
use holon_api::DynamicEntity;
use holon_api::entity::FieldSchema;
use holon_core::OperationProvider;
use holon_core::SyncTokenStore;
use holon_core::SyncableProvider;
use holon_mcp_client::mcp_sidecar::EntityConfig;
use holon_mcp_client::mcp_sidecar::McpSidecar;
use holon_mcp_client::mcp_sidecar::SyncConfig;
use holon_mcp_client::mcp_sidecar::ToolConfig;
use holon_mcp_client::mcp_sidecar::ToolEffect;
use holon_mcp_client::mcp_sync_engine::McpSyncEngine;
use rmcp::RoleClient;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::ServiceExt;
use rmcp::model::*;
use rmcp::service::Peer;
use rmcp::service::RequestContext;
use tokio::sync::RwLock;

const ENTITY_NAME: &str = "fake_probe";
const RESOURCE_URI: &str = "fake://probe/items";
const TOOLLESS_ENTITY: &str = "fake_shadow";
const READONLY_ENTITY: &str = "fake_readonly";
const WRITE_TOOL: &str = "update-probe";
const READ_TOOL: &str = "find-readonly";
const PROVIDER_NAME: &str = "fake-mcp";

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
                .enable_tools()
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

    /// The tool the sidecar classifies as `fake_probe`'s write. Its presence is
    /// what gives the connector an operation descriptor on that entity, which
    /// is what makes the connector — not a derived SQL provider — the entity's
    /// write authority.
    fn list_tools(
        &self,
        _: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        let tool = |name: &str, description: &str| Tool {
            name: name.to_string().into(),
            title: None,
            description: Some(description.to_string().into()),
            input_schema: Arc::new(serde_json::Map::new()),
            output_schema: None,
            annotations: None,
            icons: None,
            meta: None,
        };
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![
            tool(WRITE_TOOL, "Update a fake probe item"),
            tool(READ_TOOL, "List fake read-only items"),
        ])))
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
    operation_provider: holon_mcp_client::McpOperationProvider,
    _server_peer: Peer<RoleServer>,
    _items: Arc<RwLock<Vec<serde_json::Value>>>,
}

impl FakeMcpHandle {
    /// Seed the registry with the sidecar's entity types, the step
    /// `McpIntegrationsModule` runs on every connected integration. Without it
    /// a mirrored entity is invisible to everything that reasons over the
    /// registry, so no test could see what a real connector does to it.
    fn register_entity_types(&self, type_registry: &holon_profiles::TypeRegistry) {
        holon_mcp_client::register_sidecar_entity_types(
            self.sync_engine.sidecar(),
            PROVIDER_NAME,
            type_registry,
        )
        .expect("[FakeMcp] sidecar entity types register");
    }
}

/// The connector's own operations, exactly as `McpIntegrationsModule` publishes
/// them: descriptors derived from the server's tool list crossed with the
/// sidecar's `tools:` classification. Registering the handle itself as the
/// provider is also what keeps the pipeline alive for the session.
#[async_trait::async_trait]
impl OperationProvider for FakeMcpHandle {
    fn operations(&self) -> Vec<holon_api::OperationDescriptor> {
        self.operation_provider.operations()
    }
    async fn execute_operation(
        &self,
        entity: &holon_api::EntityName,
        op: &str,
        params: holon_core::storage::types::StorageEntity,
    ) -> holon::core::traits::Result<holon_core::OperationResult> {
        self.operation_provider
            .execute_operation(entity, op, params)
            .await
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
                project: Default::default(),
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
    // Two mirrored entities the connector does NOT write: one it declares no
    // tool for at all, one it declares only a READ tool for. Neither has an
    // authority of its own, so the boot sequence must still derive one from
    // their columns.
    for entity in [TOOLLESS_ENTITY, READONLY_ENTITY] {
        entities.insert(
            entity.to_string(),
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
                sync: None,
                vtable: None,
                profile_variants: vec![],
            },
        );
    }

    let mut tools = HashMap::new();
    tools.insert(
        WRITE_TOOL.to_string(),
        ToolConfig {
            entity: Some(ENTITY_NAME.to_string()),
            affected_fields: Some(vec!["data".to_string()]),
            effect: Some(ToolEffect::Idempotent),
            ..Default::default()
        },
    );
    tools.insert(
        READ_TOOL.to_string(),
        ToolConfig {
            entity: Some(READONLY_ENTITY.to_string()),
            effect: Some(ToolEffect::Read),
            ..Default::default()
        },
    );

    let sidecar = McpSidecar {
        entity_prefix: None,
        entities,
        writes: Default::default(),
        once_only: Default::default(),
        tools,
        views: vec![],
    };

    let entity_config = &sidecar.entities[ENTITY_NAME];
    let entity = sidecar.prefixed_name(ENTITY_NAME);
    let table_name = entity.table_name();
    let td = entity_config
        .to_type_definition(
            &table_name,
            PROVIDER_NAME,
            sidecar.write_ownership(ENTITY_NAME),
        )
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

    let operation_provider = holon_mcp_client::McpOperationProvider::from_peer_shared(
        client_peer.clone(),
        sidecar.clone(),
        HashMap::new(),
    )
    .await
    .map_err(|e| anyhow::anyhow!("Failed to build the fake MCP operation provider: {e}"))?;

    let sync_engine = Arc::new(McpSyncEngine::new(
        Arc::new(client_peer.clone()),
        Some(client_peer),
        strategies,
        caches,
        token_store,
        PROVIDER_NAME.to_string(),
        sidecar,
        vec![],
        Some(db_handle),
    ));

    sync_engine.sync_all().await?;
    sync_engine.subscribe_all().await?;
    let (sync_event_tx, sync_event_rx) = tokio::sync::mpsc::unbounded_channel();
    holon_mcp_client::spawn_sync_event_loop(
        sync_event_rx,
        sync_engine.clone(),
        holon_mcp_client::SyncGate::opened(),
        holon_mcp_client::SyncLoopTuning::test(),
    );
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
        operation_provider,
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
        let handle = build_handle(db_handle)
            .await
            .expect("[FakeMcp] failed to build in-memory MCP integration");
        handle.register_entity_types(&resolver.resolve::<holon_profiles::TypeRegistry>());
        Shared::new(handle)
    }));

    injector.provide_into_set::<dyn SyncableProvider>(Provider::root_async(
        |resolver| async move {
            let handle = resolver.resolve_async::<FakeMcpHandle>().await;
            handle.sync_engine.clone() as Arc<dyn SyncableProvider>
        },
    ));

    injector.provide_into_set::<dyn OperationProvider>(Provider::root_async(
        |resolver| async move {
            resolver.resolve_async::<FakeMcpHandle>().await as Arc<dyn OperationProvider>
        },
    ));
}

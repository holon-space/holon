//! A fake external MCP server the production integration module connects to.
//!
//! [`FakeMcpPeer`] serves it over streamable HTTP on a loopback port, and
//! [`FakeMcpPeer::install`] writes its sidecar into a config directory and
//! enables it, so a session booted over that directory connects it like any
//! user integration: `McpIntegrationsModule` → rmcp HTTP client →
//! `McpSyncEngine` → `QueryableCache` → Turso. The drawn
//! [`IntegrationConnectTiming`] decides when the peer starts answering.

use std::path::Path;
use std::sync::Arc;

use axum::response::IntoResponse;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;
use holon_pbt_core::capabilities::IntegrationConnectTiming;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::model::*;
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::tower::StreamableHttpServerConfig;
use rmcp::transport::streamable_http_server::tower::StreamableHttpService;
use tokio::sync::watch;

/// The integration's provider name: its sidecar file stem.
pub const PROVIDER_NAME: &str = "fake-mcp";
/// The entity the connector writes, and the operation it writes it with.
pub const WRITTEN_ENTITY: &str = "fk_fake_probe";
pub const WRITE_OP: &str = "update_probe";
/// Every entity the sidecar mirrors.
pub const ENTITIES: [&str; 3] = ["fk_fake_probe", "fk_fake_shadow", "fk_fake_readonly"];

const RESOURCE_URI: &str = "fake://probe/items";
const WRITE_TOOL: &str = "update-probe";
const READ_TOOL: &str = "find-readonly";

/// An MCP server serving one empty JSON resource and two tools.
#[derive(Clone)]
struct FakeMcpServer;

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

    async fn list_resources(
        &self,
        _: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
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

    async fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        if request.uri != RESOURCE_URI {
            return Err(ErrorData::resource_not_found("Unknown resource", None));
        }
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::text("[]", RESOURCE_URI)],
        })
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

/// The sidecar the integration module loads. Prefixed on purpose: an
/// unprefixed sidecar whose entity keys have no underscores is the one corner
/// where the raw key, the canonical EntityName and the table name coincide, so
/// every lookup succeeds by accident and no test can see them diverge.
///
/// `fake_shadow` (no tool) and `fake_readonly` (a read tool only) are mirrored
/// but not written by the connector, so the boot must derive a writer for them
/// from their columns.
fn sidecar_yaml(uri: &str) -> String {
    format!(
        r#"
schema_version: {version}
display_name: "Fake MCP"
transport:
  http:
    uri: "{uri}"
entity_prefix: "fk_"
entities:
  fake_probe:
    id_column: id
    schema:
      - {{ name: id,   sql_type: TEXT, primary_key: true }}
      - {{ name: data, sql_type: TEXT }}
    sync:
      list_resource: {RESOURCE_URI}
  fake_shadow:
    id_column: id
    schema:
      - {{ name: id,   sql_type: TEXT, primary_key: true }}
      - {{ name: data, sql_type: TEXT }}
  fake_readonly:
    id_column: id
    schema:
      - {{ name: id,   sql_type: TEXT, primary_key: true }}
      - {{ name: data, sql_type: TEXT }}
tools:
  {WRITE_TOOL}:
    entity: fake_probe
    effect: idempotent
    affected_fields: [data]
  {READ_TOOL}:
    entity: fake_readonly
    effect: read
"#,
        version = holon_mcp_client::SIDECAR_SCHEMA_VERSION,
    )
}

/// A loopback HTTP server hosting [`FakeMcpServer`]. Every request waits at
/// the gate until it opens; the server stops when the peer is dropped.
pub struct FakeMcpPeer {
    uri: String,
    timing: IntegrationConnectTiming,
    gate: watch::Sender<bool>,
    server: tokio::task::JoinHandle<()>,
}

impl FakeMcpPeer {
    pub async fn start(timing: IntegrationConnectTiming) -> Self {
        let (gate, gate_rx) = watch::channel(timing == IntegrationConnectTiming::Instant);
        let service: StreamableHttpService<FakeMcpServer, LocalSessionManager> =
            StreamableHttpService::new(
                || Ok(FakeMcpServer),
                Arc::new(LocalSessionManager::default()),
                StreamableHttpServerConfig::default(),
            );
        let app =
            axum::Router::new()
                .nest_service("/mcp", service)
                .layer(axum::middleware::from_fn(
                    move |request: axum::extract::Request, next: axum::middleware::Next| {
                        let mut gate = gate_rx.clone();
                        async move {
                            // A dropped peer leaves its in-flight connections
                            // running; they answer that the peer is gone.
                            let released = gate.wait_for(|open| *open).await.is_ok();
                            if released {
                                next.run(request).await
                            } else {
                                axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response()
                            }
                        }
                    },
                ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the fake MCP peer");
        let uri = format!(
            "http://{}/mcp",
            listener.local_addr().expect("fake MCP peer address")
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app)
                .await
                .expect("the fake MCP peer's server failed");
        });
        Self {
            uri,
            timing,
            gate,
            server,
        }
    }

    pub fn timing(&self) -> IntegrationConnectTiming {
        self.timing
    }

    /// Write the sidecar into `<config_dir>/integrations` and enable it.
    pub fn install(&self, config_dir: &Path) {
        let integrations_dir = config_dir.join("integrations");
        std::fs::create_dir_all(&integrations_dir).expect("create the integrations dir");
        std::fs::write(
            integrations_dir.join(format!("{PROVIDER_NAME}.yaml")),
            sidecar_yaml(&self.uri),
        )
        .expect("install the fake MCP sidecar");
        IntegrationConfigStore::load(&integrations_dir)
            .expect("load the integration config store")
            .set(
                PROVIDER_NAME,
                IntegrationState {
                    enabled: true,
                    configuration: Configuration::Unconfigured,
                },
            )
            .expect("enable the fake MCP integration");
    }

    /// Open the gate. Fails loud unless the peer was drawn to wait for this.
    pub fn release(&self) {
        assert_eq!(
            self.timing,
            IntegrationConnectTiming::AfterSessionResolve,
            "only a peer drawn to answer after the session resolved is released"
        );
        let was_open = self.gate.send_replace(true);
        assert!(!was_open, "the fake MCP peer was released twice");
    }
}

impl Drop for FakeMcpPeer {
    fn drop(&mut self) {
        self.server.abort();
    }
}

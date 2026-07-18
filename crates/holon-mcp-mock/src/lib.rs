//! A configurable mock MCP server that reproduces the challenging behaviours
//! seen in real MCP servers, for E2E-testing holon's generic MCP connector.
//!
//! The server speaks real MCP over stdio (rmcp `transport-io`) so it is driven
//! exactly like any production server through the `transport-child-process`
//! path. Behaviour is selected by the [`Scenario`] parsed from the
//! `MOCK_MCP_SCENARIO` env var (set in a sidecar's `child_process.env`).
//!
//! Catalogue and rationale: `docs/Plans/McpMockE2E.md`.

use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::model::*;
use rmcp::service::RequestContext;
use tokio::sync::Mutex;

/// The single tool the mock exposes for tool-based sync scenarios.
pub const LIST_TOOL: &str = "list-items";
/// The single resource URI for resource-based sync scenarios.
pub const RESOURCE_URI: &str = "mock://items";

/// Which challenging behaviour the server exhibits. See the catalogue doc.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Well-formed tool sync: `list-items` returns 3 items as a JSON block.
    Happy,
    /// `call_tool` sleeps before responding (high-latency server).
    Slow,
    /// One tool response carries 1500 records (large result set).
    Large,
    /// Cursor pagination: page size 2 over 5 items, echoing a `nextCursor`.
    Paginated,
    /// Tool returns a non-JSON text block (malformed payload).
    MalformedJson,
    /// Tool returns valid JSON but omits the documented `items` array.
    MissingField,
    /// Tool returns a JSON block AND a trailing prose block (Todoist quirk).
    DualTextBlock,
    /// Tool returns `CallToolResult::error` (`isError: true`).
    ToolError,
    /// Resource content changes between successive reads (poll-based drift).
    Stateful,
    /// Advertises `resources.subscribe` and pushes an `updated` notification.
    SubscribePush,
}

impl Scenario {
    pub fn parse(s: &str) -> anyhow::Result<Self> {
        Ok(match s {
            "happy" => Self::Happy,
            "slow" => Self::Slow,
            "large" => Self::Large,
            "paginated" => Self::Paginated,
            "malformed_json" => Self::MalformedJson,
            "missing_field" => Self::MissingField,
            "dual_text_block" => Self::DualTextBlock,
            "tool_error" => Self::ToolError,
            "stateful" => Self::Stateful,
            "subscribe_push" => Self::SubscribePush,
            other => anyhow::bail!("unknown MOCK_MCP_SCENARIO '{other}'"),
        })
    }

    /// Read the scenario from the `MOCK_MCP_SCENARIO` env var (fail-loud).
    pub fn from_env() -> anyhow::Result<Self> {
        let raw = std::env::var("MOCK_MCP_SCENARIO")
            .map_err(|_| anyhow::anyhow!("MOCK_MCP_SCENARIO env var not set"))?;
        Self::parse(&raw)
    }
}

/// Build one `{id,title,body}` item.
fn item(n: usize) -> serde_json::Value {
    serde_json::json!({
        "id": format!("i{n}"),
        "title": format!("Item {n}"),
        "body": format!("Body of item {n}"),
    })
}

/// The mock server handler. `read_count`/`items` back the stateful and push
/// scenarios; all others are pure functions of the request.
pub struct MockServer {
    scenario: Scenario,
    /// Read counter for `Stateful` (returns more rows on later reads).
    read_count: Arc<Mutex<u64>>,
    /// Live resource state for `SubscribePush` (mutated by the push task).
    items: Arc<Mutex<Vec<serde_json::Value>>>,
}

impl MockServer {
    pub fn new(scenario: Scenario) -> Self {
        Self {
            scenario,
            read_count: Arc::new(Mutex::new(0)),
            items: Arc::new(Mutex::new(vec![item(1)])),
        }
    }
}

impl ServerHandler for MockServer {
    fn get_info(&self) -> ServerInfo {
        // The builder is typestate-encoded, so each branch must build in one
        // expression rather than reassigning across calls.
        let capabilities = match self.scenario {
            Scenario::SubscribePush => ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
                .build(),
            Scenario::Stateful => ServerCapabilities::builder().enable_resources().build(),
            _ => ServerCapabilities::builder().enable_tools().build(),
        };
        ServerInfo {
            capabilities,
            server_info: Implementation {
                name: "holon-mock-mcp".into(),
                title: None,
                version: "0.1.0".into(),
                icons: None,
                website_url: None,
            },
            ..Default::default()
        }
    }

    fn list_tools(
        &self,
        _: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        let tool = Tool {
            name: LIST_TOOL.into(),
            title: None,
            description: Some("List mock items".into()),
            input_schema: Arc::new(serde_json::Map::new()),
            output_schema: None,
            annotations: None,
            icons: None,
            meta: None,
        };
        std::future::ready(Ok(ListToolsResult::with_all_items(vec![tool])))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParam,
        _: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, McpError>> + Send + '_ {
        let scenario = self.scenario;
        async move {
            if request.name != LIST_TOOL {
                return Err(McpError::invalid_params(
                    format!("unknown tool '{}'", request.name),
                    None,
                ));
            }
            if scenario == Scenario::Slow {
                tokio::time::sleep(std::time::Duration::from_millis(300)).await;
            }
            Ok(tool_response(scenario, request.arguments.as_ref()))
        }
    }

    fn list_resources(
        &self,
        _: Option<PaginatedRequestParam>,
        _: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, McpError>> + Send + '_ {
        let resource = Annotated::new(
            RawResource {
                uri: RESOURCE_URI.to_string(),
                name: "Mock Items".to_string(),
                title: None,
                description: Some("Mutable mock items resource".to_string()),
                mime_type: Some("application/json".to_string()),
                size: None,
                icons: None,
                meta: None,
            },
            None,
        );
        std::future::ready(Ok(ListResourcesResult::with_all_items(vec![resource])))
    }

    fn read_resource(
        &self,
        request: ReadResourceRequestParam,
        _: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, McpError>> + Send + '_ {
        let scenario = self.scenario;
        let read_count = self.read_count.clone();
        let items = self.items.clone();
        async move {
            if request.uri != RESOURCE_URI {
                return Err(McpError::resource_not_found("unknown resource", None));
            }
            let body = match scenario {
                Scenario::Stateful => {
                    let mut c = read_count.lock().await;
                    *c += 1;
                    // First read: 1 item. Later reads: 2 items (server drift).
                    let n = if *c <= 1 { 1 } else { 2 };
                    let rows: Vec<_> = (1..=n).map(item).collect();
                    serde_json::to_string(&rows).unwrap()
                }
                Scenario::SubscribePush => {
                    let rows = items.lock().await;
                    serde_json::to_string(&*rows).unwrap()
                }
                _ => serde_json::to_string(&vec![item(1)]).unwrap(),
            };
            Ok(ReadResourceResult {
                contents: vec![ResourceContents::text(body, RESOURCE_URI)],
            })
        }
    }

    fn subscribe(
        &self,
        _: SubscribeRequestParam,
        ctx: RequestContext<RoleServer>,
    ) -> impl std::future::Future<Output = Result<(), McpError>> + Send + '_ {
        let scenario = self.scenario;
        let items = self.items.clone();
        async move {
            if scenario == Scenario::SubscribePush {
                let peer = ctx.peer.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
                    // Mutate server state BEFORE notifying so the client's
                    // resync reads the grown resource.
                    items.lock().await.push(item(2));
                    let _ = peer
                        .notify_resource_updated(ResourceUpdatedNotificationParam {
                            uri: RESOURCE_URI.to_string(),
                        })
                        .await;
                });
            }
            Ok(())
        }
    }
}

/// Build the tool response body for a scenario, honouring the incoming cursor
/// argument for pagination.
fn tool_response(
    scenario: Scenario,
    args: Option<&serde_json::Map<String, serde_json::Value>>,
) -> CallToolResult {
    match scenario {
        Scenario::Happy | Scenario::Slow => {
            // Slow's sleep is applied by the binary before we get here.
            let rows: Vec<_> = (1..=3).map(item).collect();
            CallToolResult::success(vec![Content::text(
                serde_json::json!({ "items": rows }).to_string(),
            )])
        }
        Scenario::Large => {
            let rows: Vec<_> = (1..=1500).map(item).collect();
            CallToolResult::success(vec![Content::text(
                serde_json::json!({ "items": rows }).to_string(),
            )])
        }
        Scenario::Paginated => paginated_response(args),
        Scenario::MalformedJson => {
            CallToolResult::success(vec![Content::text("this is not json {{{".to_string())])
        }
        Scenario::MissingField => CallToolResult::success(vec![Content::text(
            serde_json::json!({ "other": [item(1)] }).to_string(),
        )]),
        Scenario::DualTextBlock => CallToolResult::success(vec![
            Content::text(serde_json::json!({ "items": [item(1), item(2)] }).to_string()),
            Content::text("Here are your 2 items — hope that helps!".to_string()),
        ]),
        Scenario::ToolError => CallToolResult::error(vec![Content::text(
            "domain error: rate limit exceeded".to_string(),
        )]),
        // Resource-based scenarios never reach call_tool.
        Scenario::Stateful | Scenario::SubscribePush => CallToolResult::error(vec![Content::text(
            "call_tool not supported for resource-based scenario".to_string(),
        )]),
    }
}

/// Cursor pagination: 5 items, page size 2. The cursor is a string offset; the
/// terminal page still echoes a cursor so the client stays on the incremental
/// (append) path and never wipes earlier pages via a cursorless full-sync.
fn paginated_response(args: Option<&serde_json::Map<String, serde_json::Value>>) -> CallToolResult {
    const TOTAL: usize = 5;
    const PAGE: usize = 2;
    let offset: usize = match args.and_then(|a| a.get("cursor")).and_then(|v| v.as_str()) {
        // The cursor is server-produced, so a non-numeric value is a mock bug.
        Some(s) => s.parse().expect("mock cursor must be a numeric string"),
        None => 0,
    };
    let end = (offset + PAGE).min(TOTAL);
    let rows: Vec<_> = (offset..end).map(|i| item(i + 1)).collect();
    let next = end.min(TOTAL);
    CallToolResult::success(vec![Content::text(
        serde_json::json!({ "items": rows, "nextCursor": next.to_string() }).to_string(),
    )])
}

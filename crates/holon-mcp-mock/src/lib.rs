//! A configurable mock MCP server that reproduces the challenging behaviours
//! seen in real MCP servers, for E2E-testing holon's generic MCP connector.
//!
//! The server speaks real MCP over stdio (rmcp `transport-io`) so it is driven
//! exactly like any production server through the `transport-child-process`
//! path. Behaviour is selected by the [`Scenario`] parsed from the
//! `MOCK_MCP_SCENARIO` env var (set in a sidecar's `child_process.env`).
//!
//! Catalogue and rationale: `docs/Plans/McpMockE2E.md`.

use std::collections::HashSet;
use std::sync::Arc;

use rmcp::ErrorData as McpError;
use rmcp::RoleServer;
use rmcp::ServerHandler;
use rmcp::model::*;
use rmcp::service::RequestContext;
use tokio::sync::Mutex;

/// The single tool the mock exposes for tool-based sync scenarios.
pub const LIST_TOOL: &str = "list-items";
/// The write tool the mock exposes for write scenarios.
pub const WRITE_TOOL: &str = "write-item";
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
    /// Write tool accepts the call and acks success (well-formed write).
    WriteHappy,
    /// Write tool dedups on the idempotency key: the first call with a key
    /// applies (bumps `applied_count`); repeats echo `deduped: true` and leave
    /// `applied_count` unchanged. The dedup authority for retry-storm tests.
    WriteDuplicateDetected,
    /// Write tool returns a CAS/precondition failure (`isError: true`).
    WriteConflict,
    /// Write tool accepts but delays its ack (slow-acking server).
    WriteSlowAck,
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
            "write_happy" => Self::WriteHappy,
            "write_duplicate_detected" => Self::WriteDuplicateDetected,
            "write_conflict" => Self::WriteConflict,
            "write_slow_ack" => Self::WriteSlowAck,
            other => anyhow::bail!("unknown MOCK_MCP_SCENARIO '{other}'"),
        })
    }

    /// Read the scenario from the `MOCK_MCP_SCENARIO` env var (fail-loud).
    pub fn from_env() -> anyhow::Result<Self> {
        let raw = std::env::var("MOCK_MCP_SCENARIO")
            .map_err(|_| anyhow::anyhow!("MOCK_MCP_SCENARIO env var not set"))?;
        Self::parse(&raw)
    }

    /// Whether this scenario exercises the write tool (advertises `write-item`
    /// instead of `list-items`).
    pub fn is_write(&self) -> bool {
        matches!(
            self,
            Self::WriteHappy
                | Self::WriteDuplicateDetected
                | Self::WriteConflict
                | Self::WriteSlowAck
        )
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
    /// Idempotency keys already applied (`WriteDuplicateDetected` dedup set).
    seen_keys: Arc<Mutex<HashSet<String>>>,
    /// Count of non-deduped writes actually applied (retry-storm assertion).
    applied_count: Arc<Mutex<u64>>,
}

impl MockServer {
    pub fn new(scenario: Scenario) -> Self {
        Self {
            scenario,
            read_count: Arc::new(Mutex::new(0)),
            items: Arc::new(Mutex::new(vec![item(1)])),
            seen_keys: Arc::new(Mutex::new(HashSet::new())),
            applied_count: Arc::new(Mutex::new(0)),
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
        let (name, description) = if self.scenario.is_write() {
            (WRITE_TOOL, "Write a mock item")
        } else {
            (LIST_TOOL, "List mock items")
        };
        let tool = Tool {
            name: name.into(),
            title: None,
            description: Some(description.into()),
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
        let seen_keys = self.seen_keys.clone();
        let applied_count = self.applied_count.clone();
        async move {
            if scenario.is_write() {
                if request.name != WRITE_TOOL {
                    return Err(McpError::invalid_params(
                        format!("unknown write tool '{}'", request.name),
                        None,
                    ));
                }
                if scenario == Scenario::WriteSlowAck {
                    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
                }
                return Ok(write_response(
                    scenario,
                    request.arguments.as_ref(),
                    &seen_keys,
                    &applied_count,
                )
                .await);
            }
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
        // Resource-based and write scenarios never reach this read helper.
        Scenario::Stateful
        | Scenario::SubscribePush
        | Scenario::WriteHappy
        | Scenario::WriteDuplicateDetected
        | Scenario::WriteConflict
        | Scenario::WriteSlowAck => CallToolResult::error(vec![Content::text(
            "tool_response is read-only; this scenario is handled elsewhere".to_string(),
        )]),
    }
}

/// Build the write-tool response for a write scenario. `WriteDuplicateDetected`
/// is the dedup authority: it keys off the incoming `idempotency_key` argument,
/// applying once and echoing `deduped: true` on repeats so a retry storm of N
/// identical dispatches yields exactly one applied effect.
async fn write_response(
    scenario: Scenario,
    args: Option<&serde_json::Map<String, serde_json::Value>>,
    seen_keys: &Mutex<HashSet<String>>,
    applied_count: &Mutex<u64>,
) -> CallToolResult {
    match scenario {
        Scenario::WriteHappy | Scenario::WriteSlowAck => {
            let mut c = applied_count.lock().await;
            *c += 1;
            CallToolResult::success(vec![Content::text(
                serde_json::json!({ "ok": true, "applied_count": *c }).to_string(),
            )])
        }
        Scenario::WriteDuplicateDetected => {
            let key = args
                .and_then(|a| a.get("idempotency_key"))
                .and_then(|v| v.as_str())
                .expect("WriteDuplicateDetected requires an idempotency_key argument")
                .to_string();
            let deduped = !seen_keys.lock().await.insert(key.clone());
            let applied = {
                let mut c = applied_count.lock().await;
                if !deduped {
                    *c += 1;
                }
                *c
            };
            CallToolResult::success(vec![Content::text(
                serde_json::json!({
                    "ok": true,
                    "deduped": deduped,
                    "applied_count": applied,
                    "key": key,
                })
                .to_string(),
            )])
        }
        Scenario::WriteConflict => CallToolResult::error(vec![Content::text(
            "precondition failed: CAS conflict — resource changed since read".to_string(),
        )]),
        _ => unreachable!("write_response called for non-write scenario {scenario:?}"),
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

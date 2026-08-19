//! @c4 component
//! @c4 layer Testing
//! Pattern: Test Harness
//!
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
/// The claude-history provider's message-sending tool, named exactly as the
/// real provider names it so the SHIPPED sidecar's declaration binds.
pub const SEND_TOOL: &str = "send_message";
/// The claude-history provider's question-answering tool.
pub const ANSWER_TOOL: &str = "answer_question";
/// The single resource URI for resource-based sync scenarios.
pub const RESOURCE_URI: &str = "mock://items";
/// The live-fleet resource of the claude-history provider.
pub const LIVE_URI: &str = "claude-history://live";
/// The pending-questions resource of the claude-history provider.
pub const QUESTIONS_URI: &str = "claude-history://live/questions";
/// The projects resource of the claude-history provider. `LiveFleet` serves it
/// empty: the claude-history sidecar syncs it alongside `live`, and an
/// unhandled URI is a hard read_resource error.
pub const PROJECTS_URI: &str = "claude-history://projects";

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
    /// Serves the claude-history provider's `live` fleet listing: the first
    /// read carries a running background session plus a foreground one, later
    /// reads drop the background one — the provider omits a session once its
    /// job reaches a terminal state.
    LiveFleet,
    /// `LiveFleet`'s resources PLUS the provider's `send_message` tool, which
    /// bumps `applied_count` on every call. The counter is cumulative across
    /// the connection, so a test proves "the effect fired exactly N times" by
    /// reading it from the Nth response. Acks `outcome: delivered` — the
    /// provider's word for "this provably landed in the session".
    LiveFleetSend,
    /// `LiveFleetSend`, except the tool acks `outcome: unconfirmed`: a
    /// SUCCESSFUL MCP response that states delivery is not proven. The common
    /// outcome for a busy session, and the one a client must not read as
    /// delivery.
    LiveFleetSendUnconfirmed,
    /// `LiveFleet`'s resources PLUS the provider's `answer_question` tool. The
    /// tool refuses a label the question does not offer (as the real provider
    /// does) and otherwise bumps `applied_count` and echoes the recorded label.
    LiveFleetAnswer,
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
    /// Write tool APPLIES the effect (bumps `applied_count`) but then returns
    /// `isError: true` — models a post-dispatch failure / lost ack: the remote
    /// side effect happened exactly once, yet the client sees an error. The
    /// once_only at-most-once test asserts the intent lands in
    /// `OutcomeUnknown` and is never auto-retried (amendment A).
    WriteAcceptedThenError,
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
            "live_fleet" => Self::LiveFleet,
            "live_fleet_send" => Self::LiveFleetSend,
            "live_fleet_send_unconfirmed" => Self::LiveFleetSendUnconfirmed,
            "live_fleet_answer" => Self::LiveFleetAnswer,
            "write_happy" => Self::WriteHappy,
            "write_duplicate_detected" => Self::WriteDuplicateDetected,
            "write_conflict" => Self::WriteConflict,
            "write_slow_ack" => Self::WriteSlowAck,
            "write_accepted_then_error" => Self::WriteAcceptedThenError,
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
                | Self::WriteAcceptedThenError
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

/// The claude-history `live` listing as of read number `read_count`: the
/// background session is present only on the first read.
///
/// `id` follows the real provider's two shapes rather than an invented one: a
/// backgrounded session is keyed by its JOB id, an attached one by `pid-<pid>`.
/// Nothing here is keyed by `session_id` — the transcript uuid is data on the
/// row, never an address, and `send_message` cannot resolve it.
fn live_fleet(read_count: u64) -> Vec<serde_json::Value> {
    let foreground = serde_json::json!({
        "id": "pid-4242",
        "kind": "session",
        "session_id": "s-fg-1",
        "project_id": "p-holon",
        "cwd": "/w/holon",
        "name": "main session",
        "state": "running",
        "status": "waiting-on-user",
        "tempo": "idle",
        "needs": "input",
        "detail": "awaiting a prompt",
        "started_at": "2026-08-03T09:00:00Z",
        "updated_at": "2026-08-03T09:31:00Z",
        "pid": 4242,
        "job_id": serde_json::Value::Null,
        "cli_version": "2.1.0",
        "tokens": 51200,
    });
    if read_count > 1 {
        return vec![foreground];
    }
    let background = serde_json::json!({
        "id": "job-77",
        "kind": "bg",
        "session_id": "s-bg-1",
        "project_id": "p-holon",
        "cwd": "/w/holon/lane",
        "name": "lane-chatinput",
        "state": "running",
        "status": "working",
        "tempo": "steady",
        "needs": "nothing",
        "detail": "editing claude-history.yaml",
        "started_at": "2026-08-03T09:10:00Z",
        "updated_at": "2026-08-03T09:30:00Z",
        "pid": serde_json::Value::Null,
        "job_id": "job-77",
        "cli_version": "2.1.0",
        "tokens": 8192,
    });
    vec![background, foreground]
}

/// The provider's declared `send_message` input schema, argument-for-argument.
fn send_tool_schema() -> serde_json::Map<String, serde_json::Value> {
    let serde_json::Value::Object(schema) = serde_json::json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description":
                    "live_session id from claude-history://live (a background short id)",
            },
            "text": { "type": "string", "description": "The message to send" },
        },
        "required": ["id", "text"],
    }) else {
        unreachable!("the literal above is an object")
    };
    schema
}

/// The real `send_message` contract, transcribed from the provider binary
/// (`claude-code-history-mcp`, `server.rs`): two required string arguments,
/// `id` and `text`, with `id` resolved against the LIVE listing by `id` or
/// `job_id` — never by `session_id`.
///
/// `Err` carries the provider's own rejection text verbatim, because a mock
/// that rejects with a message of its own invention lets a caller pass here and
/// fail against the real binary. Extra arguments are ignored, as the real
/// server ignores them.
fn check_send_contract(args: &serde_json::Map<String, serde_json::Value>) -> Result<(), String> {
    for key in ["id", "text"] {
        if !args.get(key).is_some_and(serde_json::Value::is_string) {
            return Err(format!(
                "cannot answer: {key} is required and must be a string"
            ));
        }
    }
    let id = args["id"].as_str().expect("checked above");
    let addressable = live_fleet(1)
        .iter()
        .any(|s| s["id"] == id || s["job_id"] == id);
    if !addressable {
        return Err(format!(
            "job {id} is not known to the daemon; it may have exited"
        ));
    }
    Ok(())
}

/// The claude-history `live/questions` listing: what the fleet is blocked on.
/// `id` carries the provider's opaque `question_id`
/// (`<job_id>:<question_index>:<fingerprint>`), and only the head question of a
/// session is `answerable`.
fn live_questions() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "id": "job-77:0:1a2b3c4d",
            "session_id": "s-bg-1",
            "job_id": "job-77",
            "question_index": 0,
            "question": "Which storage backend should the lane use?",
            "multi_select": 0,
            "options": [
                {"label": "Turso", "description": "embedded, IVM available"},
                {"label": "Plain SQLite", "description": "no incremental views"},
            ],
            "answerable": 1,
            "asked_at": "2026-08-03T09:29:00Z",
        }),
        serde_json::json!({
            "id": "job-77:1:99887766",
            "session_id": "s-bg-1",
            "job_id": "job-77",
            "question_index": 1,
            "question": "Ship behind a flag?",
            "multi_select": 0,
            "options": [{"label": "Yes"}, {"label": "No"}],
            "answerable": 0,
            "asked_at": "2026-08-03T09:29:30Z",
        }),
    ]
}

/// The `live/questions` row a `question_id` names, or `None` when the id no
/// longer resolves.
fn pending_question(question_id: &str) -> Option<serde_json::Value> {
    live_questions()
        .into_iter()
        .find(|q| q["id"] == question_id)
}

/// The provider's declared input schema for `answer_question`, transcribed from
/// `list_tools` in the binary. `answers` is an ARRAY of labels — a caller that
/// sends a scalar is rejected, so the schema is what a sidecar's param names
/// AND shapes must agree with.
fn answer_tool_schema() -> serde_json::Map<String, serde_json::Value> {
    let serde_json::Value::Object(schema) = serde_json::json!({
        "type": "object",
        "properties": {
            "question_id": {
                "type": "string",
                "description": "pending_question id from claude-history://live/questions",
            },
            "answers": {
                "type": "array",
                "items": { "type": "string" },
                "minItems": 1,
                "description":
                    "Chosen option labels, spelled exactly as offered; several for a \
                     multi-select question",
            },
        },
        "required": ["question_id", "answers"],
    }) else {
        unreachable!("the literal above is an object")
    };
    schema
}

/// The real `answer_question` contract, transcribed from the provider binary
/// (`claude-code-history-mcp`: `server.rs::answer_question` +
/// `steer::compose_answer`): a string `question_id` and an ARRAY of option
/// labels, joined with `", "` — the join is what makes several labels parse as
/// several selections.
///
/// `Ok` carries the composed answer text the provider would record. `Err`
/// carries the provider's own rejection text verbatim, because a mock that
/// rejects with wording of its own invention lets a caller pass here and die
/// against the real binary. Extra arguments are ignored, as the real server
/// ignores them.
fn check_answer_contract(
    args: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, String> {
    let Some(question_id) = args.get("question_id").and_then(|v| v.as_str()) else {
        return Err("cannot answer: question_id is required and must be a string".to_string());
    };
    let raw = args
        .get("answers")
        .and_then(|a| a.as_array())
        .ok_or_else(|| "cannot answer: answers must be an array of option labels".to_string())?;
    let mut answers: Vec<String> = Vec::with_capacity(raw.len());
    for value in raw {
        let label = value.as_str().ok_or_else(|| {
            format!("cannot answer: every answer must be an option label; got {value}")
        })?;
        answers.push(label.to_string());
    }

    let question = pending_question(question_id).ok_or_else(|| {
        format!(
            "question {question_id} is no longer pending; it was answered or replaced. Re-read \
             claude-history://live/questions and use the id it reports now"
        )
    })?;

    if answers.is_empty() {
        return Err("cannot answer: no options were chosen".to_string());
    }
    if question["answerable"] == 0 {
        return Err(format!(
            "cannot answer: question {} cannot be answered on its own: a reply lands in the first \
             question's slot. Answer question 0, or use send_message to reply in prose",
            question["question_index"]
        ));
    }

    let offered: Vec<&str> = question["options"]
        .as_array()
        .expect("fixture options are an array")
        .iter()
        .map(|o| o["label"].as_str().expect("fixture labels are strings"))
        .collect();
    for answer in &answers {
        if !offered.contains(&answer.as_str()) {
            return Err(format!(
                "cannot answer: {answer:?} is not one of {offered:?}"
            ));
        }
        if answer.contains(", ") {
            return Err(format!(
                "cannot answer: label {answer:?} contains \", \", which the join cannot express"
            ));
        }
    }
    let mut seen = answers.clone();
    seen.sort();
    seen.dedup();
    if seen.len() != answers.len() {
        return Err("cannot answer: the same option was chosen twice".to_string());
    }

    Ok(answers.join(", "))
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
            Scenario::Stateful | Scenario::LiveFleet => {
                ServerCapabilities::builder().enable_resources().build()
            }
            Scenario::LiveFleetSend
            | Scenario::LiveFleetSendUnconfirmed
            | Scenario::LiveFleetAnswer => ServerCapabilities::builder()
                .enable_resources()
                .enable_tools()
                .build(),
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
        let (name, description) = match self.scenario {
            Scenario::LiveFleetSend | Scenario::LiveFleetSendUnconfirmed => {
                (SEND_TOOL, "Send a message to a live session")
            }
            Scenario::LiveFleetAnswer => (ANSWER_TOOL, "Answer a pending question"),
            s if s.is_write() => (WRITE_TOOL, "Write a mock item"),
            _ => (LIST_TOOL, "List mock items"),
        };
        // Only the steering tools publish a schema: they are the ones the
        // sidecar's param names and shapes must agree with, so the mock declares
        // them exactly as the real provider does.
        let input_schema = match name {
            SEND_TOOL => send_tool_schema(),
            ANSWER_TOOL => answer_tool_schema(),
            _ => serde_json::Map::new(),
        };
        let tool = Tool {
            name: name.into(),
            title: None,
            description: Some(description.into()),
            input_schema: Arc::new(input_schema),
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
            if matches!(
                scenario,
                Scenario::LiveFleetSend | Scenario::LiveFleetSendUnconfirmed
            ) {
                if request.name != SEND_TOOL {
                    return Err(McpError::invalid_params(
                        format!("{scenario:?} serves no tool '{}'", request.name),
                        None,
                    ));
                }
                let args = request.arguments.as_ref().ok_or_else(|| {
                    McpError::invalid_params("send_message requires arguments", None)
                })?;
                if let Err(rejection) = check_send_contract(args) {
                    return Ok(CallToolResult::error(vec![Content::text(rejection)]));
                }
                let outcome = if scenario == Scenario::LiveFleetSend {
                    "delivered"
                } else {
                    "unconfirmed"
                };
                let mut c = applied_count.lock().await;
                *c += 1;
                // The arguments are echoed so a test can assert WHAT crossed
                // the connector boundary: a real provider declares its tool's
                // arguments, and anything Holon-internal must not appear here.
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({
                        "ok": true,
                        "outcome": outcome,
                        "applied_count": *c,
                        "arguments": request.arguments,
                    })
                    .to_string(),
                )]));
            }
            if scenario == Scenario::LiveFleetAnswer {
                if request.name != ANSWER_TOOL {
                    return Err(McpError::invalid_params(
                        format!("LiveFleetAnswer serves no tool '{}'", request.name),
                        None,
                    ));
                }
                let args = request.arguments.as_ref().ok_or_else(|| {
                    McpError::invalid_params("answer_question requires arguments", None)
                })?;
                let composed = match check_answer_contract(args) {
                    Ok(composed) => composed,
                    Err(rejection) => {
                        return Ok(CallToolResult::error(vec![Content::text(rejection)]));
                    }
                };
                let mut c = applied_count.lock().await;
                *c += 1;
                // `recorded` is the COMPOSED text, which for a multi-select is
                // the `", "`-joined labels — what the dialog actually stores.
                return Ok(CallToolResult::success(vec![Content::text(
                    serde_json::json!({
                        "outcome": "recorded",
                        "recorded": composed,
                        "applied_count": *c,
                        "arguments": request.arguments,
                    })
                    .to_string(),
                )]));
            }
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
            if matches!(
                scenario,
                Scenario::LiveFleet
                    | Scenario::LiveFleetSend
                    | Scenario::LiveFleetSendUnconfirmed
                    | Scenario::LiveFleetAnswer
            ) {
                let body = match request.uri.as_str() {
                    PROJECTS_URI => "[]".to_string(),
                    QUESTIONS_URI => serde_json::to_string(&live_questions()).unwrap(),
                    LIVE_URI => {
                        let mut c = read_count.lock().await;
                        *c += 1;
                        serde_json::to_string(&live_fleet(*c)).unwrap()
                    }
                    other => {
                        return Err(McpError::resource_not_found(
                            format!("{scenario:?} serves no resource '{other}'"),
                            None,
                        ));
                    }
                };
                return Ok(ReadResourceResult {
                    contents: vec![ResourceContents::text(body, request.uri)],
                });
            }
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
        | Scenario::LiveFleet
        | Scenario::LiveFleetSend
        | Scenario::LiveFleetSendUnconfirmed
        | Scenario::LiveFleetAnswer
        | Scenario::WriteHappy
        | Scenario::WriteDuplicateDetected
        | Scenario::WriteConflict
        | Scenario::WriteSlowAck
        | Scenario::WriteAcceptedThenError => CallToolResult::error(vec![Content::text(
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
        Scenario::WriteAcceptedThenError => {
            // Apply the effect exactly once, THEN fail the ack: the remote side
            // effect happened but the client sees an error (amendment A).
            let mut c = applied_count.lock().await;
            *c += 1;
            CallToolResult::error(vec![Content::text(
                "post-dispatch failure: write applied on the server but the ack was lost"
                    .to_string(),
            )])
        }
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

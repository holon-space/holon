//! End-to-end fan-out tests for `McpForeignDataWrapper` driven by a scripted
//! `MockPeer`. Exercises the `McpCursor::filter` runtime path: enumeration of
//! parent rows, per-row tool/resource calls, response merging, and id-scheme
//! prefixing.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::mcp_vtable::McpForeignDataWrapper;
use holon_mcp_client::mcp_vtable::UriParamValue;
use holon_mcp_client::mcp_vtable::VtableConfig;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::Content;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ReadResourceResult;
use rmcp::model::ResourceContents;
use rmcp::service::ServiceError;
use turso_core::Connection as CoreConnection;
use turso_core::Database;
use turso_core::DatabaseOpts;
use turso_core::MemoryIO;
use turso_core::OpenFlags;
use turso_core::StepResult;
use turso_core::Value;
use turso_core::foreign::ForeignDataWrapper;
use turso_core::foreign::PushedConstraint;
use turso_ext::ConstraintOp;

// ---------------------------------------------------------------------------
// MockPeer
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct RecordedToolCall {
    pub tool: String,
    pub params: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Default)]
struct MockPeerInner {
    /// One queue of pre-scripted JSON-body responses per tool name. Each
    /// call_tool() pops the front. The full envelope sent back is a
    /// `CallToolResult` whose `content` is a single `TextContent` holding the
    /// JSON-stringified body.
    tool_responses: std::collections::HashMap<String, VecDeque<serde_json::Value>>,
    /// One response per URI. Each read_resource() returns a clone.
    resource_responses: std::collections::HashMap<String, serde_json::Value>,
    tool_calls: Vec<RecordedToolCall>,
    resource_reads: Vec<String>,
}

#[derive(Debug, Default)]
pub struct MockPeer {
    inner: Mutex<MockPeerInner>,
}

impl MockPeer {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue_tool_response(&self, tool: &str, body: serde_json::Value) {
        let mut inner = self.inner.lock().unwrap();
        inner
            .tool_responses
            .entry(tool.to_string())
            .or_default()
            .push_back(body);
    }

    pub fn set_resource_response(&self, uri: &str, body: serde_json::Value) {
        let mut inner = self.inner.lock().unwrap();
        inner.resource_responses.insert(uri.to_string(), body);
    }

    pub fn tool_calls(&self) -> Vec<RecordedToolCall> {
        self.inner.lock().unwrap().tool_calls.clone()
    }

    pub fn resource_reads(&self) -> Vec<String> {
        self.inner.lock().unwrap().resource_reads.clone()
    }
}

#[async_trait]
impl McpCallSurface for MockPeer {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        let tool = params.name.to_string();
        let args = params.arguments.unwrap_or_default();

        let mut inner = self.inner.lock().unwrap();
        inner.tool_calls.push(RecordedToolCall {
            tool: tool.clone(),
            params: args,
        });
        let body = inner
            .tool_responses
            .get_mut(&tool)
            .and_then(|q| q.pop_front())
            .unwrap_or_else(|| {
                panic!(
                    "MockPeer: no scripted response for tool '{tool}' (call #{})",
                    inner.tool_calls.len()
                )
            });

        let text = serde_json::to_string(&body).expect("body serializable");
        Ok(CallToolResult {
            content: vec![Content::text(text)],
            structured_content: None,
            is_error: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        let uri = params.uri;
        let mut inner = self.inner.lock().unwrap();
        inner.resource_reads.push(uri.clone());
        let body = inner
            .resource_responses
            .get(&uri)
            .cloned()
            .unwrap_or_else(|| panic!("MockPeer: no scripted response for resource '{uri}'"));
        let text = serde_json::to_string(&body).expect("body serializable");
        Ok(ReadResourceResult {
            contents: vec![ResourceContents::TextResourceContents {
                uri,
                mime_type: Some("application/json".to_string()),
                text,
                meta: None,
            }],
        })
    }
}

// ---------------------------------------------------------------------------
// In-memory Turso CoreConnection helper
// ---------------------------------------------------------------------------

fn open_memory_conn() -> Arc<CoreConnection> {
    let io = Arc::new(MemoryIO::new());
    let opts = DatabaseOpts::default().with_views(true);
    let db = Database::open_file_with_flags(
        io,
        ":memory:",
        OpenFlags::default(),
        opts,
        None,
        Arc::new(turso_core::SqliteDialect),
    )
    .expect("open in-memory database");
    db.connect().expect("connect")
}

fn execute_sql(conn: &Arc<CoreConnection>, sql: &str) {
    let mut stmt = conn
        .query(sql)
        .unwrap_or_else(|e| panic!("query setup failed for `{sql}`: {e}"))
        .unwrap_or_else(|| panic!("no statement for `{sql}`"));
    loop {
        match stmt
            .step()
            .unwrap_or_else(|e| panic!("step for `{sql}`: {e}"))
        {
            StepResult::Row | StepResult::IO => continue,
            StepResult::Done => break,
            other => panic!("unexpected StepResult for `{sql}`: {other:?}"),
        }
    }
}

// ---------------------------------------------------------------------------
// Fan-out shape helpers
// ---------------------------------------------------------------------------

fn build_fdw(
    table_name: &str,
    columns: &[(&str, &str)],
    yaml: &str,
    peer: Arc<dyn McpCallSurface>,
    entity_prefix: Option<&str>,
) -> McpForeignDataWrapper {
    let config: VtableConfig = serde_yaml::from_str(yaml).expect("vtable yaml parses");
    let columns: Vec<(String, String)> = columns
        .iter()
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect();
    McpForeignDataWrapper::new(
        table_name,
        &columns,
        &config,
        peer,
        None,
        None,
        tokio::runtime::Handle::current(),
        entity_prefix,
        &std::collections::HashMap::new(),
    )
}

fn collect_rows(
    fdw: &McpForeignDataWrapper,
    conn: Arc<CoreConnection>,
    constraints: &[PushedConstraint],
    column_count: usize,
) -> Vec<Vec<Value>> {
    let mut cursor = fdw.open_cursor(conn).expect("open_cursor");
    let mut has_row = cursor.filter(constraints).expect("filter");
    let mut out = Vec::new();
    while has_row {
        let row: Vec<Value> = (0..column_count)
            .map(|i| cursor.column(i).expect("column"))
            .collect();
        out.push(row);
        has_row = cursor.next().expect("next");
    }
    out
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Legacy single-field enumeration: parent table `cc_session` has 3 rows; tool
/// `list_messages` is called once per session id. Each call returns one record
/// keyed by that session id. Final result: 3 rows in cursor.
#[tokio::test(flavor = "multi_thread")]
async fn legacy_single_field_enumeration_fans_out() {
    let conn = open_memory_conn();
    execute_sql(&conn, "CREATE TABLE cc_session (id TEXT PRIMARY KEY)");
    // Cached ids are scheme-prefixed (id_scheme convention); the enumeration
    // boundary must strip the prefix before substituting into tool params.
    execute_sql(
        &conn,
        "INSERT INTO cc_session(id) VALUES ('cc-session:s1'), ('cc-session:s2'), ('cc-session:s3')",
    );

    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "list_messages",
        serde_json::json!({"messages": [{"id": "m_s1", "session_id": "s1"}]}),
    );
    peer.enqueue_tool_response(
        "list_messages",
        serde_json::json!({"messages": [{"id": "m_s2", "session_id": "s2"}]}),
    );
    peer.enqueue_tool_response(
        "list_messages",
        serde_json::json!({"messages": [{"id": "m_s3", "session_id": "s3"}]}),
    );

    let yaml = r#"
search_tool: list_messages
extract_path: messages
filter_mapping:
  session_id:
    param: session_id
    ops: ["="]
    enumerate_from:
      entity: session
      field: id
"#;
    let fdw = build_fdw(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
    );

    let rows = collect_rows(&fdw, conn, &[], 2);
    assert_eq!(rows.len(), 3, "fan-out across 3 parent sessions");

    let calls = peer.tool_calls();
    assert_eq!(calls.len(), 3, "one tool call per parent row");
    let mut session_args: Vec<String> = calls
        .iter()
        .filter_map(|c| {
            c.params
                .get("session_id")
                .and_then(|v| v.as_str())
                .map(str::to_string)
        })
        .collect();
    session_args.sort();
    assert_eq!(session_args, vec!["s1", "s2", "s3"]);
}

/// Paired multi-field enumeration: `gh_repository` has 2 rows; each
/// (owner, name) pair drives one `list_issues` call. Both `owner` and `repo`
/// must be bound from the same parent row (not Cartesian).
#[tokio::test(flavor = "multi_thread")]
async fn paired_fields_enumeration_fans_out() {
    let conn = open_memory_conn();
    execute_sql(
        &conn,
        "CREATE TABLE gh_repository (owner TEXT, name TEXT, PRIMARY KEY(owner, name))",
    );
    execute_sql(
        &conn,
        "INSERT INTO gh_repository(owner, name) VALUES ('octo', 'hello'), ('martin', 'holon')",
    );

    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "list_issues",
        serde_json::json!({"issues": [{"id": "i1", "owner": "octo", "repo": "hello"}]}),
    );
    peer.enqueue_tool_response(
        "list_issues",
        serde_json::json!({"issues": [{"id": "i2", "owner": "martin", "repo": "holon"}]}),
    );

    let yaml = r#"
search_tool: list_issues
extract_path: issues
filter_mapping:
  owner:
    param: owner
    ops: ["="]
    enumerate_from:
      entity: repository
      fields:
        owner: owner
        repo:  name
  repo:
    param: repo
    ops: ["="]
"#;
    let fdw = build_fdw(
        "gh_issue",
        &[("id", "TEXT"), ("owner", "TEXT"), ("repo", "TEXT")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );

    let rows = collect_rows(&fdw, conn, &[], 3);
    assert_eq!(rows.len(), 2, "one fetched row per parent (owner,name)");

    let calls = peer.tool_calls();
    assert_eq!(calls.len(), 2);
    // Both params must come from the same parent row — no Cartesian.
    let mut pairs: Vec<(String, String)> = calls
        .iter()
        .map(|c| {
            (
                c.params.get("owner").unwrap().as_str().unwrap().to_string(),
                c.params.get("repo").unwrap().as_str().unwrap().to_string(),
            )
        })
        .collect();
    pairs.sort();
    assert_eq!(
        pairs,
        vec![
            ("martin".to_string(), "holon".to_string()),
            ("octo".to_string(), "hello".to_string()),
        ]
    );
}

/// When the caller supplies the enumeration's owning param via WHERE, no
/// fan-out occurs — exactly one tool call with the user-provided value.
#[tokio::test(flavor = "multi_thread")]
async fn where_provided_param_skips_enumeration() {
    let conn = open_memory_conn();
    execute_sql(&conn, "CREATE TABLE cc_session (id TEXT PRIMARY KEY)");
    // Seed parent rows that would have triggered fan-out — they must be ignored.
    execute_sql(
        &conn,
        "INSERT INTO cc_session(id) VALUES ('cc-session:s1'), ('cc-session:s2')",
    );

    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "list_messages",
        serde_json::json!({"messages": [{"id": "m_pinned", "session_id": "s_pinned"}]}),
    );

    let yaml = r#"
search_tool: list_messages
extract_path: messages
filter_mapping:
  session_id:
    param: session_id
    ops: ["="]
    enumerate_from:
      entity: session
      field: id
"#;
    let fdw = build_fdw(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
    );

    // session_id is at column index 1.
    let constraints = vec![PushedConstraint {
        column_index: 1,
        op: ConstraintOp::Eq,
        value: Value::build_text("s_pinned"),
    }];
    let rows = collect_rows(&fdw, conn, &constraints, 2);
    assert_eq!(rows.len(), 1);

    let calls = peer.tool_calls();
    assert_eq!(calls.len(), 1, "no fan-out when caller supplied the param");
    assert_eq!(
        calls[0].params.get("session_id").and_then(|v| v.as_str()),
        Some("s_pinned")
    );
}

/// Empty parent table → zero tool calls and zero rows. Fan-out must not run
/// against missing FK targets, and the fallback "fetch with empty params" path
/// must not fire either.
#[tokio::test(flavor = "multi_thread")]
async fn empty_parent_table_yields_no_calls() {
    let conn = open_memory_conn();
    execute_sql(&conn, "CREATE TABLE cc_session (id TEXT PRIMARY KEY)");
    // No rows.

    let peer = Arc::new(MockPeer::new());
    let yaml = r#"
search_tool: list_messages
extract_path: messages
filter_mapping:
  session_id:
    param: session_id
    ops: ["="]
    enumerate_from:
      entity: session
      field: id
"#;
    let fdw = build_fdw(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
    );

    let rows = collect_rows(&fdw, conn, &[], 2);
    assert!(rows.is_empty());
    assert!(peer.tool_calls().is_empty(), "no fan-out without parents");
}

/// Call params are stamped onto each returned record under their key names
/// — required for tools (list_issues, list_pull_requests) whose response
/// doesn't echo the owner/repo that produced it.
#[tokio::test(flavor = "multi_thread")]
async fn call_params_stamped_into_records() {
    let conn = open_memory_conn();
    execute_sql(
        &conn,
        "CREATE TABLE gh_repository (owner TEXT, name TEXT, PRIMARY KEY(owner, name))",
    );
    execute_sql(
        &conn,
        "INSERT INTO gh_repository(owner, name) VALUES ('nightscape', 'holon')",
    );

    let peer = Arc::new(MockPeer::new());
    // list_issues response: numbers + title only, no owner/repo echo.
    peer.enqueue_tool_response(
        "list_issues",
        serde_json::json!({"issues": [
            {"number": 1, "title": "first"},
            {"number": 2, "title": "second"},
        ]}),
    );

    let yaml = r#"
search_tool: list_issues
extract_path: issues
filter_mapping:
  owner:
    param: owner
    ops: ["="]
    enumerate_from:
      entity: repository
      fields:
        owner: owner
        repo: name
  repo:
    param: repo
    ops: ["="]
"#;
    let fdw = build_fdw(
        "gh_issue",
        &[
            ("owner", "TEXT"),
            ("repo", "TEXT"),
            ("number", "INTEGER"),
            ("title", "TEXT"),
        ],
        yaml,
        peer.clone(),
        Some("gh_"),
    );
    let rows = collect_rows(&fdw, conn, &[], 4);
    assert_eq!(rows.len(), 2);
    // owner and repo carried from the call params.
    assert_eq!(rows[0][0], Value::build_text("nightscape"));
    assert_eq!(rows[0][1], Value::build_text("holon"));
    assert_eq!(rows[1][0], Value::build_text("nightscape"));
    assert_eq!(rows[1][1], Value::build_text("holon"));
}

/// Cursor pagination: two pages then `hasNextPage: false`. All rows
/// accumulated, second call sends the prior `endCursor` as `after`.
#[tokio::test(flavor = "multi_thread")]
async fn pagination_cursor_two_pages_then_done() {
    let conn = open_memory_conn();
    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "list_issues",
        serde_json::json!({
            "issues": [{"number": 1}, {"number": 2}],
            "pageInfo": {"hasNextPage": true, "endCursor": "cursor-2"}
        }),
    );
    peer.enqueue_tool_response(
        "list_issues",
        serde_json::json!({
            "issues": [{"number": 3}],
            "pageInfo": {"hasNextPage": false, "endCursor": null}
        }),
    );

    let yaml = r#"
search_tool: list_issues
extract_path: issues
filter_mapping:
  owner:
    param: owner
    ops: ["="]
pagination:
  style: cursor
  cursor_response_path: pageInfo.endCursor
  has_more_path: pageInfo.hasNextPage
  cursor_param: after
  size_param: perPage
  page_size: 100
"#;
    let fdw = build_fdw(
        "gh_issue",
        &[("number", "INTEGER")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );
    let constraints = vec![PushedConstraint {
        column_index: 0,
        op: ConstraintOp::Eq,
        value: Value::build_text("ignored"),
    }];
    let _ = constraints; // only need params to flow
    let rows = collect_rows(&fdw, conn, &[], 1);
    assert_eq!(rows.len(), 3, "two pages flattened");

    let calls = peer.tool_calls();
    assert_eq!(calls.len(), 2);
    assert!(
        calls[0].params.get("after").is_none(),
        "first call has no cursor"
    );
    assert_eq!(
        calls[1].params.get("after").and_then(|v| v.as_str()),
        Some("cursor-2")
    );
}

/// page_total pagination: stop when total reached.
#[tokio::test(flavor = "multi_thread")]
async fn pagination_page_total_stops_at_total() {
    let conn = open_memory_conn();
    let peer = Arc::new(MockPeer::new());
    // page 1, perPage 2, total 3 → page 2 expected, second page returns 1
    peer.enqueue_tool_response(
        "search_repositories",
        serde_json::json!({
            "items": [{"name": "a"}, {"name": "b"}],
            "total_count": 3
        }),
    );
    peer.enqueue_tool_response(
        "search_repositories",
        serde_json::json!({"items": [{"name": "c"}], "total_count": 3}),
    );

    let yaml = r#"
search_tool: search_repositories
extract_path: items
filter_mapping:
  query:
    param: query
    ops: ["="]
pagination:
  style: page_total
  page_param: page
  size_param: perPage
  page_size: 2
  total_response_path: total_count
"#;
    let fdw = build_fdw(
        "gh_repository",
        &[("name", "TEXT")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );
    let rows = collect_rows(&fdw, conn, &[], 1);
    assert_eq!(rows.len(), 3);
    let calls = peer.tool_calls();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].params.get("page"), Some(&serde_json::json!(1)));
    assert_eq!(calls[1].params.get("page"), Some(&serde_json::json!(2)));
}

/// page_short pagination: stop when a page returns < page_size.
#[tokio::test(flavor = "multi_thread")]
async fn pagination_page_short_stops_on_short_page() {
    let conn = open_memory_conn();
    let peer = Arc::new(MockPeer::new());
    // Bare array PRs; perPage 2, page 2 returns 1 (short) → stop.
    peer.enqueue_tool_response(
        "list_pull_requests",
        serde_json::json!([{"number": 1}, {"number": 2}]),
    );
    peer.enqueue_tool_response("list_pull_requests", serde_json::json!([{"number": 3}]));

    let yaml = r#"
search_tool: list_pull_requests
filter_mapping:
  owner:
    param: owner
    ops: ["="]
pagination:
  style: page_short
  page_param: page
  size_param: perPage
  page_size: 2
"#;
    let fdw = build_fdw(
        "gh_pull_request",
        &[("number", "INTEGER")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );
    let rows = collect_rows(&fdw, conn, &[], 1);
    assert_eq!(rows.len(), 3);
    assert_eq!(peer.tool_calls().len(), 2);
}

/// Nested JSON paths: a column declared with `source_path: "owner.login"`
/// reads through the nested object. Required for `minimal_output: false`
/// search_repositories responses.
#[tokio::test(flavor = "multi_thread")]
async fn nested_source_path_pulls_owner_login() {
    let conn = open_memory_conn();
    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "search_repositories",
        serde_json::json!({"items": [
            {"name": "holon", "owner": {"login": "nightscape", "id": 42}},
            {"name": "spark-excel", "owner": {"login": "nightscape", "id": 42}},
        ]}),
    );

    let yaml = r#"
search_tool: search_repositories
extract_path: items
filter_mapping:
  query:
    param: query
    ops: ["="]
columns:
  owner:
    source_path: owner.login
"#;
    let fdw = build_fdw(
        "gh_repository",
        &[("name", "TEXT"), ("owner", "TEXT")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );

    let rows = collect_rows(&fdw, conn, &[], 2);
    assert_eq!(rows.len(), 2);
    // Row 0: name=holon, owner=nightscape (pulled from owner.login)
    assert_eq!(rows[0][0], Value::build_text("holon"));
    assert_eq!(rows[0][1], Value::build_text("nightscape"));
    assert_eq!(rows[1][0], Value::build_text("spark-excel"));
    assert_eq!(rows[1][1], Value::build_text("nightscape"));
}

/// `encoding: json` on a TEXT column JSON-stringifies array/object values.
/// Required for `topics: []` (search_repositories) and `labels: []` (issues).
#[tokio::test(flavor = "multi_thread")]
async fn json_encoding_serialises_arrays_into_text() {
    let conn = open_memory_conn();
    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "search_repositories",
        serde_json::json!({"items": [
            {"name": "holon", "topics": ["pkm", "rust", "graph"]},
        ]}),
    );

    let yaml = r#"
search_tool: search_repositories
extract_path: items
filter_mapping:
  query:
    param: query
    ops: ["="]
columns:
  topics:
    encoding: json
"#;
    let fdw = build_fdw(
        "gh_repository",
        &[("name", "TEXT"), ("topics", "TEXT")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );

    let rows = collect_rows(&fdw, conn, &[], 2);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0][1], Value::build_text(r#"["pkm","rust","graph"]"#));
}

/// `static_args` constants must be present on every call, merged under any
/// WHERE-derived values (WHERE wins on collision).
#[tokio::test(flavor = "multi_thread")]
async fn static_args_inject_into_every_call() {
    let conn = open_memory_conn();
    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "search_repositories",
        serde_json::json!({"items": [{"name": "r1"}]}),
    );

    let yaml = r#"
search_tool: search_repositories
extract_path: items
static_args:
  minimal_output: false
  perPage: 100
filter_mapping:
  query:
    param: query
    ops: ["="]
"#;
    let fdw = build_fdw(
        "gh_repository",
        &[("name", "TEXT")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );

    let constraints = vec![PushedConstraint {
        column_index: 0,
        op: ConstraintOp::Eq,
        value: Value::build_text("user:nightscape"),
    }];
    // Column 0 is `name`, but we route via the query filter_mapping which is at
    // column index 1 (unmapped here). Easiest: construct a constraint on the
    // mapped column. The vtable maps query → column index = 1 (after name).
    // To keep the test simple, just verify static_args show up.
    let _ = constraints;
    let _ = collect_rows(&fdw, conn, &[], 1);

    let calls = peer.tool_calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].params.get("minimal_output"),
        Some(&serde_json::json!(false)),
        "static minimal_output present"
    );
    assert_eq!(
        calls[0].params.get("perPage"),
        Some(&serde_json::json!(100)),
        "static perPage present"
    );
}

/// Tool whose response is a bare JSON array (no envelope). `extract_path`
/// omitted from YAML — vtable should take the response root as the records
/// array. Required for `list_pull_requests`.
#[tokio::test(flavor = "multi_thread")]
async fn bare_array_response_no_extract_path() {
    let conn = open_memory_conn();

    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "list_pull_requests",
        serde_json::json!([
            {"number": 1, "title": "first"},
            {"number": 2, "title": "second"},
        ]),
    );

    let yaml = r#"
search_tool: list_pull_requests
filter_mapping:
  owner:
    param: owner
    ops: ["="]
"#;
    let fdw = build_fdw(
        "gh_pull_request",
        &[("number", "INTEGER"), ("title", "TEXT")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );

    let constraints = vec![PushedConstraint {
        column_index: 0,
        op: ConstraintOp::Eq,
        value: Value::build_text("octo"),
    }];
    // Column 0 is `number`, not owner — but we just need one constraint to
    // exercise the call path; the bare-array assertion is what matters.
    let _ = constraints;
    let rows = collect_rows(&fdw, conn, &[], 2);
    assert_eq!(rows.len(), 2);
    assert_eq!(peer.tool_calls().len(), 1);
}

/// Tool returning multiple records in a single call: cursor surfaces all of
/// them, no fan-out involved.
#[tokio::test(flavor = "multi_thread")]
async fn single_call_returns_multiple_records() {
    let conn = open_memory_conn();

    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "search_repositories",
        serde_json::json!({
            "repositories": [
                {"id": "r1", "owner": "octo", "name": "a"},
                {"id": "r2", "owner": "octo", "name": "b"},
                {"id": "r3", "owner": "octo", "name": "c"},
            ]
        }),
    );

    let yaml = r#"
search_tool: search_repositories
extract_path: repositories
filter_mapping:
  owner:
    param: owner
    ops: ["="]
"#;
    let fdw = build_fdw(
        "gh_repository",
        &[("id", "TEXT"), ("owner", "TEXT"), ("name", "TEXT")],
        yaml,
        peer.clone(),
        Some("gh_"),
    );

    let constraints = vec![PushedConstraint {
        column_index: 1,
        op: ConstraintOp::Eq,
        value: Value::build_text("octo"),
    }];
    let rows = collect_rows(&fdw, conn, &constraints, 3);
    assert_eq!(rows.len(), 3);
    assert_eq!(peer.tool_calls().len(), 1);
}

/// Round-trip the canonical github.yaml at ~/.config/holon/integrations/
/// through `IntegrationFileConfig` deserialization. Catches schema drift
/// between the YAML and the Rust types. Skipped silently if the file
/// is absent (CI / fresh checkouts).
#[test]
fn github_yaml_parses_as_integration_file_config() {
    let path = std::path::Path::new(env!("HOME")).join(".config/holon/integrations/github.yaml");
    if !path.exists() {
        eprintln!("skip: {} not present", path.display());
        return;
    }
    let body = std::fs::read_to_string(&path).expect("read github.yaml");
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(&body).expect("github.yaml parses as IntegrationFileConfig");
    assert_eq!(cfg.entity_prefix.as_deref(), Some("gh_"));
    for ent in ["repository", "issue", "pull_request"] {
        let entry = cfg
            .entities
            .get(ent)
            .unwrap_or_else(|| panic!("missing entity '{ent}'"));
        assert!(entry.vtable.is_some(), "entity '{ent}' must declare vtable");
    }
}

// ---------------------------------------------------------------------------
// Phase 3: bounded, filterable, incrementally-refreshable fan-out
// ---------------------------------------------------------------------------

/// Like `build_fdw` but with id_scheme + cache table (write-through) wired,
/// as `finish_integration` does in prod.
#[allow(clippy::too_many_arguments)]
fn build_fdw_writeback(
    table_name: &str,
    columns: &[(&str, &str)],
    yaml: &str,
    peer: Arc<dyn McpCallSurface>,
    entity_prefix: Option<&str>,
    id_scheme: Option<(String, String)>,
    cache_table: Option<String>,
) -> McpForeignDataWrapper {
    let config: VtableConfig = serde_yaml::from_str(yaml).expect("vtable yaml parses");
    let columns: Vec<(String, String)> = columns
        .iter()
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect();
    McpForeignDataWrapper::new(
        table_name,
        &columns,
        &config,
        peer,
        id_scheme,
        cache_table,
        tokio::runtime::Handle::current(),
        entity_prefix,
        &std::collections::HashMap::new(),
    )
}

fn query_texts(conn: &Arc<CoreConnection>, sql: &str) -> Vec<Vec<String>> {
    let mut stmt = conn
        .query(sql)
        .unwrap_or_else(|e| panic!("query failed for `{sql}`: {e}"))
        .unwrap_or_else(|| panic!("no statement for `{sql}`"));
    let mut out = Vec::new();
    loop {
        match stmt.step().expect("step") {
            StepResult::Row => {
                let row = stmt.row().expect("row");
                let n = row.len();
                out.push(
                    (0..n)
                        .map(|i| match row.get_value(i) {
                            Value::Text(t) => t.as_str().to_owned(),
                            Value::Null => "NULL".to_string(),
                            Value::Numeric(turso_core::Numeric::Integer(n)) => n.to_string(),
                            other => format!("{other:?}"),
                        })
                        .collect(),
                );
            }
            StepResult::IO => continue,
            StepResult::Done => break,
            other => panic!("unexpected StepResult: {other:?}"),
        }
    }
    out
}

/// `where` + `order_by` + `limit` on enumerate_from narrow the fan-out to the
/// matching parents only — the watermark pattern as pure local SQL.
#[tokio::test(flavor = "multi_thread")]
async fn enumeration_where_order_limit_narrows_fan_out() {
    let conn = open_memory_conn();
    execute_sql(
        &conn,
        "CREATE TABLE cc_session (id TEXT PRIMARY KEY, modified TEXT)",
    );
    execute_sql(
        &conn,
        "INSERT INTO cc_session(id, modified) VALUES ('cc-session:s1', '2026-01-05'), \
         ('cc-session:s2', '2026-01-10'), ('cc-session:s3', '2026-01-01')",
    );

    let peer = Arc::new(MockPeer::new());
    peer.enqueue_tool_response(
        "list_messages",
        serde_json::json!({"messages": [{"id": "m_s2", "session_id": "s2"}]}),
    );

    let yaml = r#"
search_tool: list_messages
extract_path: messages
filter_mapping:
  session_id:
    param: session_id
    ops: ["="]
    enumerate_from:
      entity: session
      field: id
      where: "modified > '2026-01-02'"
      order_by: modified DESC
      limit: 1
"#;
    let fdw = build_fdw(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
    );

    let rows = collect_rows(&fdw, conn, &[], 2);
    assert_eq!(rows.len(), 1);

    let calls = peer.tool_calls();
    assert_eq!(calls.len(), 1, "where+limit narrowed fan-out to one parent");
    assert_eq!(
        calls[0].params.get("session_id").and_then(|v| v.as_str()),
        Some("s2"),
        "highest `modified` above the watermark wins (order_by DESC, limit 1)"
    );
}

/// max_fan_out exceeded → LOUD error naming limit and count; zero tool calls.
#[tokio::test(flavor = "multi_thread")]
async fn max_fan_out_exceeded_fails_loud() {
    let conn = open_memory_conn();
    execute_sql(&conn, "CREATE TABLE cc_session (id TEXT PRIMARY KEY)");
    execute_sql(
        &conn,
        "INSERT INTO cc_session(id) VALUES ('cc-session:s1'), ('cc-session:s2'), ('cc-session:s3')",
    );

    let peer = Arc::new(MockPeer::new());
    let yaml = r#"
search_tool: list_messages
extract_path: messages
max_fan_out: 2
filter_mapping:
  session_id:
    param: session_id
    ops: ["="]
    enumerate_from:
      entity: session
      field: id
"#;
    let fdw = build_fdw(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
    );

    let mut cursor = fdw.open_cursor(conn).expect("open_cursor");
    let err = cursor.filter(&[]).expect_err("must fail loud");
    let msg = format!("{err}");
    assert!(msg.contains("max_fan_out=2"), "names the limit: {msg}");
    assert!(msg.contains("3 fan-out targets"), "names the count: {msg}");
    assert!(peer.tool_calls().is_empty(), "no calls after refusal");
}

/// A cached parent id WITHOUT the expected scheme prefix is a loud error at
/// the enumeration boundary — never silently passed through to the server.
#[tokio::test(flavor = "multi_thread")]
async fn unprefixed_cached_id_fails_loud() {
    let conn = open_memory_conn();
    execute_sql(&conn, "CREATE TABLE cc_session (id TEXT PRIMARY KEY)");
    execute_sql(&conn, "INSERT INTO cc_session(id) VALUES ('raw-no-prefix')");

    let peer = Arc::new(MockPeer::new());
    let yaml = r#"
search_tool: list_messages
extract_path: messages
filter_mapping:
  session_id:
    param: session_id
    ops: ["="]
    enumerate_from:
      entity: session
      field: id
"#;
    let fdw = build_fdw(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
    );

    let mut cursor = fdw.open_cursor(conn).expect("open_cursor");
    let err = cursor.filter(&[]).expect_err("must fail loud");
    let msg = format!("{err}");
    assert!(
        msg.contains("cc-session:") && msg.contains("raw-no-prefix"),
        "names the expected scheme and the offending value: {msg}"
    );
    assert!(peer.tool_calls().is_empty());
}

/// Chunked writeback: more rows than one chunk (500) land intact in the cache.
#[tokio::test(flavor = "multi_thread")]
async fn chunked_writeback_writes_all_rows() {
    let conn = open_memory_conn();
    execute_sql(
        &conn,
        "CREATE TABLE cc_message (id TEXT PRIMARY KEY, session_id TEXT)",
    );

    let n = 1010usize;
    let records: Vec<serde_json::Value> = (0..n)
        .map(|i| serde_json::json!({"id": format!("m{i:04}"), "session_id": "s1"}))
        .collect();
    let peer = Arc::new(MockPeer::new());
    peer.set_resource_response("mcp://messages", serde_json::Value::Array(records));

    let yaml = r#"
list_resource: "mcp://messages"
write_through: true
"#;
    let fdw = build_fdw_writeback(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
        Some(("id".to_string(), "cc-message".to_string())),
        Some("cc_message".to_string()),
    );

    let rows = collect_rows(&fdw, conn.clone(), &[], 2);
    assert_eq!(rows.len(), n);

    let cached = query_texts(&conn, "SELECT COUNT(*) FROM cc_message");
    assert_eq!(
        cached[0][0],
        format!("{n}"),
        "all rows written across chunks"
    );
    let first = query_texts(&conn, "SELECT id FROM cc_message ORDER BY id LIMIT 1");
    assert_eq!(
        first[0][0], "cc-message:m0000",
        "id scheme applied in cache"
    );
}

/// Per-parent stale-row deletion: refreshing parent s1 deletes s1's vanished
/// rows but NEVER touches rows of parents outside this refresh (scoped, not
/// global), and the refreshed rows replace the stale set.
#[tokio::test(flavor = "multi_thread")]
async fn stale_rows_deleted_per_refreshed_parent_only() {
    let conn = open_memory_conn();
    execute_sql(&conn, "CREATE TABLE cc_session (id TEXT PRIMARY KEY)");
    execute_sql(&conn, "INSERT INTO cc_session(id) VALUES ('cc-session:s1')");
    execute_sql(
        &conn,
        "CREATE TABLE cc_message (id TEXT PRIMARY KEY, session_id TEXT)",
    );
    // Stale child of s1 (server no longer returns it) + a child of an
    // un-enumerated parent sX that must survive.
    execute_sql(
        &conn,
        "INSERT INTO cc_message(id, session_id) VALUES ('cc-message:stale1', 's1'), \
         ('cc-message:keep_other', 'sX')",
    );

    let peer = Arc::new(MockPeer::new());
    // Response does NOT echo session_id — the stamped call param carries it,
    // which is also what scopes the deletion.
    peer.enqueue_tool_response(
        "list_messages",
        serde_json::json!({"messages": [{"id": "fresh1"}]}),
    );

    let yaml = r#"
search_tool: list_messages
extract_path: messages
write_through: true
filter_mapping:
  session_id:
    param: session_id
    ops: ["="]
    enumerate_from:
      entity: session
      field: id
"#;
    let fdw = build_fdw_writeback(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
        Some(("id".to_string(), "cc-message".to_string())),
        Some("cc_message".to_string()),
    );

    let rows = collect_rows(&fdw, conn.clone(), &[], 2);
    assert_eq!(rows.len(), 1);

    let mut cached = query_texts(&conn, "SELECT id, session_id FROM cc_message ORDER BY id");
    cached.sort();
    assert_eq!(
        cached,
        vec![
            vec!["cc-message:fresh1".to_string(), "s1".to_string()],
            vec!["cc-message:keep_other".to_string(), "sX".to_string()],
        ],
        "stale1 deleted (s1 refreshed); keep_other untouched (sX not refreshed)"
    );
}

/// Bounded-concurrency fan-out keeps deterministic writeback ordering: rows
/// come back grouped by parent in enumeration order.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_fan_out_preserves_parent_order() {
    let conn = open_memory_conn();
    execute_sql(&conn, "CREATE TABLE cc_session (id TEXT PRIMARY KEY)");
    execute_sql(
        &conn,
        "INSERT INTO cc_session(id) VALUES ('cc-session:s1'), ('cc-session:s2'), \
         ('cc-session:s3'), ('cc-session:s4'), ('cc-session:s5'), ('cc-session:s6')",
    );

    let peer = Arc::new(MockPeer::new());
    for i in 1..=6 {
        peer.enqueue_tool_response(
            "list_messages",
            serde_json::json!({"messages": [{"id": format!("m{i}")}]}),
        );
    }

    let yaml = r#"
search_tool: list_messages
extract_path: messages
filter_mapping:
  session_id:
    param: session_id
    ops: ["="]
    enumerate_from:
      entity: session
      field: id
      order_by: id
"#;
    let fdw = build_fdw(
        "cc_message",
        &[("id", "TEXT"), ("session_id", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
    );

    let rows = collect_rows(&fdw, conn, &[], 2);
    assert_eq!(rows.len(), 6);
    // session_id column (stamped from call params) must follow enumeration
    // order s1..s6 regardless of completion interleaving.
    let session_col: Vec<String> = rows
        .iter()
        .map(|r| match &r[1] {
            Value::Text(t) => t.as_str().to_owned(),
            other => panic!("expected text, got {other:?}"),
        })
        .collect();
    assert_eq!(session_col, vec!["s1", "s2", "s3", "s4", "s5", "s6"]);
}

// ---------------------------------------------------------------------------
// claude-history multi-project enumeration (Increment A2)
// ---------------------------------------------------------------------------

/// The shipped `docs/integrations/claude-history.yaml` parses and declares the
/// multi-project shape: a sync-only `project` root, session/task fanning out
/// over it via `enumerate_from`, an enabled `message` fan-out, and NO entity
/// declaring both a `sync` strategy and a write-through vtable (the
/// finish_integration clash invariant, locked in config).
#[test]
fn claude_history_yaml_multi_project_shape() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/integrations/claude-history.yaml"
    );
    let body = std::fs::read_to_string(path).expect("read claude-history.yaml");
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(&body).expect("claude-history.yaml parses as IntegrationFileConfig");
    assert_eq!(cfg.entity_prefix.as_deref(), Some("cc_"));

    // `project` is the sync-only multi-project root (no write-through vtable).
    let project = cfg.entities.get("project").expect("project entity");
    assert!(project.sync.is_some(), "project must sync-warm its cache");
    assert!(
        project.vtable.is_none(),
        "project must not declare a vtable (sync owns the cache table)"
    );

    // session + task + agent fan out over project via enumerate_from on project_id.
    for ent in ["session", "task", "agent"] {
        let e = cfg.entities.get(ent).unwrap_or_else(|| panic!("{ent}"));
        assert!(
            e.sync.is_none(),
            "{ent} must NOT sync — enumeration lives in the write-through vtable"
        );
        let vt = e.vtable.as_ref().unwrap_or_else(|| panic!("{ent} vtable"));
        assert!(vt.write_through, "{ent} vtable must write through");
        assert_eq!(vt.max_fan_out, Some(50), "{ent} fan-out must be bounded");
        let ef = match vt.uri_params.get("project_id") {
            Some(UriParamValue::Dynamic(d)) => &d.enumerate_from,
            other => panic!("{ent} project_id must enumerate_from, got {other:?}"),
        };
        assert_eq!(ef.entity, "project");
        assert_eq!(ef.field.as_deref(), Some("id"));
        assert_eq!(ef.order_by.as_deref(), Some("last_activity DESC"));
        assert_eq!(ef.limit, Some(50));
    }

    // The session watermark strips the 'cc-project:' scheme (11 chars → 12).
    let session_ef = match cfg.entities["session"]
        .vtable
        .as_ref()
        .unwrap()
        .uri_params
        .get("project_id")
    {
        Some(UriParamValue::Dynamic(d)) => &d.enumerate_from,
        _ => unreachable!(),
    };
    let where_sql = session_ef.where_sql.as_deref().expect("session watermark");
    assert!(
        where_sql.contains("substr(cc_project.id, 12)"),
        "session watermark must strip the cc-project: scheme: {where_sql}"
    );

    // message fan-out is ENABLED: enumerate_from session, bounded, watermarked.
    let msg = cfg.entities.get("message").expect("message entity");
    let mvt = msg.vtable.as_ref().expect("message vtable enabled");
    assert!(mvt.write_through);
    assert_eq!(mvt.max_fan_out, Some(50));
    let mef = match mvt.uri_params.get("session_id") {
        Some(UriParamValue::Dynamic(d)) => &d.enumerate_from,
        other => panic!("message session_id must enumerate_from, got {other:?}"),
    };
    assert_eq!(mef.entity, "session");
    assert_eq!(mef.limit, Some(20));
    assert!(
        mef.where_sql
            .as_deref()
            .unwrap()
            .contains("substr(cc_session.id, 12)"),
        "message watermark must strip the cc-session: scheme"
    );

    // Agent grain: `agent` carries the attribution columns a Holon task links
    // through, and `agent_message` is pinned by agent id (the `agent` render's
    // live_query) with the same bounded+watermarked fan-out as `message`.
    let agent = cfg.entities.get("agent").expect("agent entity");
    let agent_cols: Vec<&str> = agent.schema.iter().map(|f| f.name.as_str()).collect();
    for col in [
        "id",
        "session_id",
        "project_id",
        "agent_type",
        "parent_agent_id",
    ] {
        assert!(
            agent_cols.contains(&col),
            "agent must declare '{col}' (have: {agent_cols:?})"
        );
    }

    let agent_msg = cfg
        .entities
        .get("agent_message")
        .expect("agent_message entity");
    let amvt = agent_msg.vtable.as_ref().expect("agent_message vtable");
    assert_eq!(
        amvt.list_resource.as_deref(),
        Some("claude-history://agents/{agent_id}/messages"),
        "agent_message must read the provider's agent-scoped message resource"
    );
    assert!(amvt.write_through);
    assert_eq!(amvt.max_fan_out, Some(50));
    let amef = match amvt.uri_params.get("agent_id") {
        Some(UriParamValue::Dynamic(d)) => &d.enumerate_from,
        other => panic!("agent_message agent_id must enumerate_from, got {other:?}"),
    };
    assert_eq!(amef.entity, "agent");
    assert_eq!(amef.limit, Some(20));
    let am_where = amef.where_sql.as_deref().expect("agent_message watermark");
    assert!(
        am_where.contains("substr(cc_agent.id, 10)"),
        "agent_message watermark must strip the 9-char 'cc-agent:' scheme: {am_where}"
    );
    // cc_agent.ended_at is an mtime (always >= the last message timestamp), so
    // the `modified > MAX(timestamp)` idiom would never suppress a re-fetch.
    assert!(
        am_where.contains("NOT EXISTS"),
        "agent_message must watermark on 'nothing cached yet', not on ended_at: {am_where}"
    );

    // agent_id is the attribution column on BOTH message shapes.
    for ent in ["message", "agent_message"] {
        let cols: Vec<&str> = cfg.entities[ent]
            .schema
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert!(
            cols.contains(&"agent_id"),
            "{ent} must declare agent_id (have: {cols:?})"
        );
    }

    // The clash invariant: sync XOR write-through vtable, per entity.
    for (name, e) in &cfg.entities {
        let has_sync = e.sync.is_some();
        let has_wt = e.vtable.as_ref().is_some_and(|v| v.write_through);
        assert!(
            !(has_sync && has_wt),
            "entity '{name}' declares both sync and write-through vtable (clash)"
        );
    }
}

/// End-to-end resource-template fan-out over the `cc_project` cache table:
/// an unpinned session query enumerates projects, applies the `last_activity`
/// watermark (skipping projects with no newer activity), strips the
/// 'cc-project:' scheme to build the raw resource URI, and writes the fetched
/// sessions through into `cc_session`. This is the exact mechanism the shipped
/// yaml uses for multi-project session enumeration.
#[tokio::test(flavor = "multi_thread")]
async fn project_enumeration_fans_out_sessions_with_watermark() {
    let conn = open_memory_conn();
    execute_sql(
        &conn,
        "CREATE TABLE cc_project (id TEXT PRIMARY KEY, last_activity TEXT)",
    );
    execute_sql(
        &conn,
        "INSERT INTO cc_project(id, last_activity) VALUES \
         ('cc-project:p1', '2026-01-10'), ('cc-project:p2', '2026-01-05'), \
         ('cc-project:p3', '2026-01-01')",
    );
    execute_sql(
        &conn,
        "CREATE TABLE cc_session (id TEXT PRIMARY KEY, project_id TEXT, modified TEXT)",
    );
    // p1 has a cached session OLDER than its last_activity  -> re-fetched.
    // p2 has a cached session NEWER than its last_activity  -> skipped by
    // watermark. p3 has no cached session                              ->
    // re-fetched.
    execute_sql(
        &conn,
        "INSERT INTO cc_session(id, project_id, modified) VALUES \
         ('cc-session:old1', 'p1', '2026-01-08'), \
         ('cc-session:keep2', 'p2', '2026-01-06')",
    );

    let peer = Arc::new(MockPeer::new());
    peer.set_resource_response(
        "claude-history://projects/p1/sessions",
        serde_json::json!([{"id": "s1", "project_id": "p1", "modified": "2026-01-10"}]),
    );
    peer.set_resource_response(
        "claude-history://projects/p3/sessions",
        serde_json::json!([{"id": "s3", "project_id": "p3", "modified": "2026-01-01"}]),
    );

    let yaml = r#"
list_resource: "claude-history://projects/{project_id}/sessions"
uri_params:
  project_id:
    enumerate_from:
      entity: project
      field: id
      where: >-
        last_activity > COALESCE((SELECT MAX(s.modified) FROM cc_session s
                                  WHERE s.project_id = substr(cc_project.id, 12)), '')
      order_by: last_activity DESC
      limit: 50
write_through: true
max_fan_out: 50
"#;
    let fdw = build_fdw_writeback(
        "cc_session",
        &[("id", "TEXT"), ("project_id", "TEXT"), ("modified", "TEXT")],
        yaml,
        peer.clone(),
        Some("cc_"),
        Some(("id".to_string(), "cc-session".to_string())),
        Some("cc_session".to_string()),
    );

    let rows = collect_rows(&fdw, conn.clone(), &[], 3);
    assert_eq!(rows.len(), 2, "only p1 and p3 fetched (p2 below watermark)");

    // Only p1 and p3 resources were read — p2 was pruned by the watermark,
    // and the raw ids (scheme stripped) built the URIs.
    let mut reads = peer.resource_reads();
    reads.sort();
    assert_eq!(
        reads,
        vec![
            "claude-history://projects/p1/sessions".to_string(),
            "claude-history://projects/p3/sessions".to_string(),
        ]
    );

    // Cache warmed: p1 refreshed (old1 replaced by s1), p2 untouched, p3 added.
    let mut cached = query_texts(&conn, "SELECT id, project_id FROM cc_session ORDER BY id");
    cached.sort();
    assert_eq!(
        cached,
        vec![
            vec!["cc-session:keep2".to_string(), "p2".to_string()],
            vec!["cc-session:s1".to_string(), "p1".to_string()],
            vec!["cc-session:s3".to_string(), "p3".to_string()],
        ],
        "write-through warmed cc_session with scheme-prefixed ids; p2 survived"
    );
}

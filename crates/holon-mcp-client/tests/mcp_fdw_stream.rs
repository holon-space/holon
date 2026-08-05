//! Incremental maintenance of materialized views over MCP-backed foreign
//! tables.
//!
//! Two things must hold for a chat view to update from a `resources/updated`
//! notification instead of a rescan: the driver has to declare an identity
//! (without one the engine keeps snapshot semantics and builds no mirror), and
//! the notification has to be translated into full-width `FdwChange` rows.

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::mcp_sidecar::EntityConfig;
use holon_mcp_client::mcp_vtable::McpForeignDataWrapper;
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

// ---------------------------------------------------------------------------
// Scripted MCP peer
// ---------------------------------------------------------------------------

#[derive(Debug, Default)]
struct ScriptedPeerInner {
    tool_responses: std::collections::HashMap<String, VecDeque<serde_json::Value>>,
    resource_responses: std::collections::HashMap<String, serde_json::Value>,
    resource_reads: Vec<String>,
}

/// A peer whose resource bodies can be swapped mid-test, so "the remote
/// changed" is expressible without a live server.
#[derive(Debug, Default)]
pub struct ScriptedPeer {
    inner: Mutex<ScriptedPeerInner>,
}

impl ScriptedPeer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Install (or replace) the body served for `uri`. Replacing it is how a
    /// test says the upstream resource changed.
    pub fn set_resource_response(&self, uri: &str, body: serde_json::Value) {
        self.inner
            .lock()
            .unwrap()
            .resource_responses
            .insert(uri.to_string(), body);
    }

    pub fn resource_reads(&self) -> Vec<String> {
        self.inner.lock().unwrap().resource_reads.clone()
    }
}

#[async_trait]
impl McpCallSurface for ScriptedPeer {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        let tool = params.name.to_string();
        let mut inner = self.inner.lock().unwrap();
        let body = inner
            .tool_responses
            .get_mut(&tool)
            .and_then(|q| q.pop_front())
            .unwrap_or_else(|| panic!("ScriptedPeer: no scripted response for tool '{tool}'"));
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
            .unwrap_or_else(|| panic!("ScriptedPeer: no scripted response for resource '{uri}'"));
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
// Harness
// ---------------------------------------------------------------------------

const MESSAGES_URI: &str = "claude://messages";

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
        .unwrap_or_else(|e| panic!("prepare failed for {sql}: {e}"))
        .unwrap_or_else(|| panic!("no statement for {sql}"));
    loop {
        match stmt
            .step()
            .unwrap_or_else(|e| panic!("step failed for {sql}: {e}"))
        {
            StepResult::Row | StepResult::IO => continue,
            StepResult::Done => break,
            other => panic!("unexpected step result for {sql}: {other:?}"),
        }
    }
}

fn query_rows(conn: &Arc<CoreConnection>, sql: &str) -> Vec<Vec<Value>> {
    let mut stmt = conn
        .query(sql)
        .unwrap_or_else(|e| panic!("prepare failed for {sql}: {e}"))
        .unwrap_or_else(|| panic!("no statement for {sql}"));
    let mut out = Vec::new();
    loop {
        match stmt
            .step()
            .unwrap_or_else(|e| panic!("step failed for {sql}: {e}"))
        {
            StepResult::Row => {
                let row = stmt.row().expect("row available");
                out.push(row.get_values().cloned().collect());
            }
            StepResult::IO => continue,
            StepResult::Done => break,
            other => panic!("unexpected step result for {sql}: {other:?}"),
        }
    }
    out
}

/// A resource-mode message table: `id` is the identity, `body` the payload.
fn build_message_fdw(peer: Arc<dyn McpCallSurface>) -> McpForeignDataWrapper {
    let yaml = format!(
        r#"
list_resource: "{MESSAGES_URI}"
"#
    );
    let config: VtableConfig = serde_yaml::from_str(&yaml).expect("vtable yaml parses");
    let columns = vec![
        ("id".to_string(), "TEXT".to_string()),
        ("body".to_string(), "TEXT".to_string()),
    ];
    McpForeignDataWrapper::new(
        "cc_message_fdw",
        &columns,
        &config,
        peer,
        Some(("id".to_string(), "cc-message".to_string())),
        &["id".to_string()],
        None,
        tokio::runtime::Handle::current(),
        Some("cc_"),
        &std::collections::HashMap::new(),
    )
}

fn text(v: &Value) -> String {
    match v {
        Value::Text(t) => t.as_str().to_string(),
        other => panic!("expected text, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// The identity declaration is the switch that turns on incremental
/// maintenance: `identity_columns() == None` makes the engine build no mirror
/// and keep matviews as snapshots, so no notification can ever reach them.
/// The MCP driver knows its identity column — it is the one it scheme-prefixes.
#[tokio::test(flavor = "multi_thread")]
async fn mcp_driver_declares_its_identity_column() {
    let peer = Arc::new(ScriptedPeer::new());
    let fdw = build_message_fdw(peer);

    assert_eq!(
        fdw.identity_columns(),
        Some(&[0u32][..]),
        "the MCP driver must declare its id column as the row identity; \
         None keeps the engine on snapshot semantics and no matview over this \
         table can ever be maintained incrementally"
    );
}

/// The claude-history `message` entity, trimmed to what identity resolution
/// reads: uuid-keyed via `primary_key: true`, and NO `id_column` anywhere.
const UUID_KEYED_ENTITY_YAML: &str = r#"
vtable:
  list_resource: "claude-history://sessions/{session_id}/messages"
  uri_params:
    session_id:
      enumerate_from:
        entity: session
        field: id
  write_through: true
schema:
  - name: uuid
    sql_type: TEXT
    primary_key: true
  - name: session_id
    sql_type: TEXT
    nullable: true
  - name: content
    sql_type: TEXT
    nullable: true
"#;

/// Row identity is DECLARED by the schema (`primary_key: true`), not implied by
/// an entity omitting `id_column`: omission means "not declared", and
/// `id_column_or_default()` answers `"id"` to a question nobody asked. The real
/// claude-history message tables are uuid-keyed and name no `id_column`, so
/// reading identity off that default names a column that does not exist.
#[tokio::test(flavor = "multi_thread")]
async fn a_uuid_keyed_entity_takes_its_identity_from_the_declared_primary_key() {
    let entity: EntityConfig =
        serde_yaml::from_str(UUID_KEYED_ENTITY_YAML).expect("entity yaml parses");
    let columns: Vec<(String, String)> = entity
        .schema
        .iter()
        .map(|f| (f.name.clone(), f.sql_type.clone()))
        .collect();

    let fdw = McpForeignDataWrapper::new(
        "cc_message",
        &columns,
        entity.vtable.as_ref().expect("entity declares a vtable"),
        Arc::new(ScriptedPeer::new()),
        Some((entity.id_column_or_default(), "cc-message".to_string())),
        &entity.identity_columns(),
        None,
        tokio::runtime::Handle::current(),
        Some("cc_"),
        &std::collections::HashMap::new(),
    );

    let uuid_idx = columns
        .iter()
        .position(|(n, _)| n == "uuid")
        .expect("the schema declares uuid") as u32;
    assert_eq!(
        fdw.identity_columns(),
        Some(&[uuid_idx][..]),
        "identity must come from the column the schema marks primary_key"
    );
}

/// A second subscription silently replaced the first: the orphaned receiver
/// keeps its channel open forever while the snapshot advances past it, so its
/// mirror goes permanently stale with nothing logged and nothing returned.
#[tokio::test(flavor = "multi_thread")]
async fn a_second_subscription_is_refused_rather_than_orphaning_the_first() {
    let conn = open_memory_conn();
    let peer = Arc::new(ScriptedPeer::new());
    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "hello"}]),
    );
    let fdw = Arc::new(build_message_fdw(peer.clone()));
    conn.register_foreign_table("cc_message_fdw", fdw.clone())
        .expect("register foreign table");
    execute_sql(
        &conn,
        "CREATE MATERIALIZED VIEW cc_message_mv AS SELECT id, body FROM cc_message_fdw",
    );

    let first = fdw
        .subscribe(&[])
        .expect("the first subscription is granted");

    let err = fdw
        .subscribe(&[])
        .expect_err(
            "a second subscription must be refused: accepting it drops the first receiver's \
             sender while the snapshot keeps advancing, which is permanent silent staleness",
        )
        .to_string();
    assert!(
        err.contains("cc_message_fdw") && err.contains("subscriber"),
        "the refusal must name the table and say a subscriber already exists, got: {err}"
    );

    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "edited"}]),
    );
    let pushed = fdw
        .push_resource_update(&conn, MESSAGES_URI)
        .expect("the notification translates");
    assert!(pushed > 0, "the edit must produce a change");
    conn.drain_fdw_stream("cc_message_fdw", &first)
        .expect("the surviving subscriber still receives its changes");
    let after = query_rows(&conn, "SELECT id, body FROM cc_message_mv ORDER BY id");
    assert_eq!(text(&after[0][1]), "edited");
}

/// The deliverable: a resource change reaches a materialized view over the MCP
/// foreign table with no REFRESH.
#[tokio::test(flavor = "multi_thread")]
async fn resource_update_reaches_the_matview_without_refresh() {
    let conn = open_memory_conn();
    let peer = Arc::new(ScriptedPeer::new());
    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "hello"}]),
    );

    let fdw = Arc::new(build_message_fdw(peer.clone()));
    conn.register_foreign_table("cc_message_fdw", fdw.clone())
        .expect("register foreign table");

    execute_sql(
        &conn,
        "CREATE MATERIALIZED VIEW cc_message_mv AS SELECT id, body FROM cc_message_fdw",
    );

    let before = query_rows(&conn, "SELECT id, body FROM cc_message_mv ORDER BY id");
    assert_eq!(before.len(), 1, "the view starts with the primed row");
    assert_eq!(text(&before[0][1]), "hello");

    // The upstream resource changes, then the server notifies us about it.
    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "edited"}, {"id": "m2", "body": "new"}]),
    );

    let changes = fdw
        .subscribe(&[])
        .expect("an MCP table with an identity supports subscription");
    let pushed = fdw
        .push_resource_update(&conn, MESSAGES_URI)
        .expect("translating a resources/updated notification must not fail silently");
    assert!(
        pushed > 0,
        "the notification must translate into at least one FdwChange"
    );

    conn.drain_fdw_stream("cc_message_fdw", &changes)
        .expect("draining the change batch into the mirrors must succeed");

    let after = query_rows(&conn, "SELECT id, body FROM cc_message_mv ORDER BY id");
    assert_eq!(
        after.len(),
        2,
        "the new message must appear in the matview with NO REFRESH"
    );
    assert_eq!(
        text(&after[0][1]),
        "edited",
        "the edited message must be updated in place, not duplicated"
    );
    assert_eq!(text(&after[1][1]), "new");

    assert!(
        !peer.resource_reads().is_empty(),
        "the driver re-fetches to build full-width rows (notifications carry no row data)"
    );
}

/// Seed one clean row, build the view over it, and subscribe — the state every
/// push test starts from.
fn seeded_message_table(
    peer: &Arc<ScriptedPeer>,
) -> (
    Arc<CoreConnection>,
    Arc<McpForeignDataWrapper>,
    std::sync::mpsc::Receiver<turso_core::foreign::FdwChange>,
) {
    let conn = open_memory_conn();
    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "hello"}]),
    );
    let fdw = Arc::new(build_message_fdw(peer.clone()));
    conn.register_foreign_table("cc_message_fdw", fdw.clone())
        .expect("register foreign table");
    execute_sql(
        &conn,
        "CREATE MATERIALIZED VIEW cc_message_mv AS SELECT id, body FROM cc_message_fdw",
    );
    let changes = fdw.subscribe(&[]).expect("subscription");
    (conn, fdw, changes)
}

/// The engine refuses two rows it cannot tell apart at REFRESH (Option A). The
/// push path must refuse the same rows: a last-wins insert into the diff map
/// drops one of them, and the view then disagrees with a direct scan of the
/// very same table with nothing logged.
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_identities_are_refused_not_collapsed() {
    let peer = Arc::new(ScriptedPeer::new());
    let (conn, fdw, _changes) = seeded_message_table(&peer);

    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "first"}, {"id": "m1", "body": "second"}]),
    );

    let err = fdw
        .push_resource_update(&conn, MESSAGES_URI)
        .expect_err(
            "a scan carrying two rows with the same identity must be refused, exactly as the \
             engine refuses it at REFRESH — collapsing them silently loses a row",
        )
        .to_string();
    assert!(
        err.contains("cc_message_fdw") && err.contains("same identity"),
        "the refusal must name the table and the duplicated identity, got: {err}"
    );

    let after = query_rows(
        &conn,
        "SELECT id, body FROM cc_message_mv ORDER BY id, body",
    );
    assert_eq!(
        after.len(),
        1,
        "a refused push must stage nothing: the view still holds only the seeded row"
    );
    assert_eq!(text(&after[0][1]), "hello");
}

/// A NULL identity cannot be matched across scans, so a row carrying one can
/// never have its update or its removal propagated — the engine refuses it and
/// so must the push path.
#[tokio::test(flavor = "multi_thread")]
async fn a_null_identity_is_refused() {
    let peer = Arc::new(ScriptedPeer::new());
    let (conn, fdw, _changes) = seeded_message_table(&peer);

    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "hello"}, {"id": null, "body": "orphan"}]),
    );

    let err = fdw
        .push_resource_update(&conn, MESSAGES_URI)
        .expect_err("a row whose declared identity is NULL must be refused")
        .to_string();
    assert!(
        err.contains("cc_message_fdw") && err.contains("NULL"),
        "the refusal must name the table and the NULL identity, got: {err}"
    );

    let after = query_rows(&conn, "SELECT id, body FROM cc_message_mv ORDER BY id");
    assert_eq!(
        after.len(),
        1,
        "a refused push must stage nothing, not the rows that happened to be valid"
    );
}

/// The snapshot models what the MIRROR holds, not what the last scan returned.
/// An ordinary `SELECT` over the foreign table re-scans upstream but leaves the
/// mirror alone, so letting it advance the snapshot would make the next
/// notification diff current-against-current, emit nothing, and strand the
/// matview stale with no error at all.
#[tokio::test(flavor = "multi_thread")]
async fn an_interleaved_scan_does_not_swallow_the_next_update() {
    let conn = open_memory_conn();
    let peer = Arc::new(ScriptedPeer::new());
    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "hello"}]),
    );

    let fdw = Arc::new(build_message_fdw(peer.clone()));
    conn.register_foreign_table("cc_message_fdw", fdw.clone())
        .expect("register foreign table");

    execute_sql(
        &conn,
        "CREATE MATERIALIZED VIEW cc_message_mv AS SELECT id, body FROM cc_message_fdw",
    );

    let changes = fdw.subscribe(&[]).expect("subscription");

    peer.set_resource_response(
        MESSAGES_URI,
        serde_json::json!([{"id": "m1", "body": "edited"}, {"id": "m2", "body": "new"}]),
    );

    // The interleaved ad-hoc scan: it reads upstream but writes no mirror.
    let direct = query_rows(&conn, "SELECT id, body FROM cc_message_fdw ORDER BY id");
    assert_eq!(
        direct.len(),
        2,
        "the ad-hoc scan sees the new upstream state (it is a live read)"
    );

    let pushed = fdw
        .push_resource_update(&conn, MESSAGES_URI)
        .expect("translating the notification must not fail");
    assert!(
        pushed > 0,
        "an interleaved scan must not swallow the update: the mirror still holds the OLD rows, \
         so the notification still has real changes to carry"
    );

    conn.drain_fdw_stream("cc_message_fdw", &changes)
        .expect("draining the batch");

    let after = query_rows(&conn, "SELECT id, body FROM cc_message_mv ORDER BY id");
    assert_eq!(
        after.len(),
        2,
        "the matview must reflect the change even though a plain SELECT ran in between"
    );
    assert_eq!(text(&after[0][1]), "edited");
    assert_eq!(text(&after[1][1]), "new");
}

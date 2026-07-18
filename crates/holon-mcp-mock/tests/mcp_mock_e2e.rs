//! End-to-end tests of holon's generic MCP connector against the configurable
//! mock MCP server (`mock-mcp-server`), driven over REAL stdio through the
//! production `build_mcp_integration` entry point.
//!
//! Each test points a YAML sidecar's `child_process` transport at the built
//! mock binary, selects a challenging behaviour via `MOCK_MCP_SCENARIO`, then
//! either queries the resulting cache table (happy paths) or asserts a loud
//! error (protocol/data violations). Catalogue: `docs/Plans/McpMockE2E.md`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::di::DbHandleCacheFactory;
use holon_api::StreamPosition;
use holon_api::Value;
use holon_core::SyncTokenStore;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpConnectionResult;
use holon_mcp_client::McpIntegration;
use holon_mcp_client::PendingOAuthFlows;
use holon_mcp_client::SyncGate;
use holon_mcp_client::build_mcp_integration;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

const MOCK_BIN: &str = env!("CARGO_BIN_EXE_mock-mcp-server");
const RAW_BIN: &str = env!("CARGO_BIN_EXE_mock-mcp-raw");
const TABLE: &str = "mock_items";

// ── In-memory SyncTokenStore ──────────────────────────────────────
struct InMemorySyncTokenStore {
    tokens: tokio::sync::Mutex<HashMap<String, StreamPosition>>,
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
    async fn save_token(&self, key: &str, position: StreamPosition) -> holon_core::Result<()> {
        self.tokens.lock().await.insert(key.to_string(), position);
        Ok(())
    }
    async fn load_token(&self, key: &str) -> holon_core::Result<Option<StreamPosition>> {
        Ok(self.tokens.lock().await.get(key).cloned())
    }
    async fn clear_all_tokens(&self) -> holon_core::Result<()> {
        self.tokens.lock().await.clear();
        Ok(())
    }
}

// ── Harness helpers ───────────────────────────────────────────────
async fn setup_db() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    // Leak the backend so the actor outlives the test.
    std::mem::forget(backend);
    handle
}

fn load_fixture(name: &str) -> IntegrationFileConfig {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    let yaml = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
    serde_yaml::from_str(&yaml).unwrap_or_else(|e| panic!("parse {path}: {e}"))
}

/// Build an integration from a fixture, overriding the command with `bin` and
/// (optionally) inserting the scenario into the child env.
async fn connect(
    fixture: &str,
    bin: &str,
    scenario: Option<&str>,
    db: &DbHandle,
) -> anyhow::Result<McpConnectionResult> {
    let mut cfg = load_fixture(fixture);
    let cp = cfg
        .transport
        .child_process
        .as_mut()
        .expect("fixture must declare child_process transport");
    cp.command = bin.to_string();
    if let Some(s) = scenario {
        cp.env
            .insert("MOCK_MCP_SCENARIO".to_string(), s.to_string());
    }
    let mcp_config = cfg.into_mcp_config("mock".to_string())?;
    let cache_factory = Arc::new(DbHandleCacheFactory::new(db.clone()));
    let token_store: Arc<dyn SyncTokenStore> = Arc::new(InMemorySyncTokenStore::new());
    let pending = PendingOAuthFlows::new();
    build_mcp_integration(
        mcp_config,
        db.clone(),
        cache_factory,
        token_store,
        &pending,
        SyncGate::opened(),
    )
    .await
}

fn connected(result: McpConnectionResult) -> McpIntegration {
    match result {
        McpConnectionResult::Connected(integration) => integration,
        McpConnectionResult::NeedsAuth { provider_name, .. } => {
            panic!("unexpected NeedsAuth for provider '{provider_name}'")
        }
    }
}

async fn count_rows(db: &DbHandle, table: &str) -> i64 {
    let rows = db
        .query(
            &format!("SELECT COUNT(*) AS c FROM {table}"),
            HashMap::new(),
        )
        .await
        .expect("count query");
    match rows[0].get("c") {
        Some(Value::Integer(n)) => *n,
        other => panic!("COUNT column: unexpected value {other:?}"),
    }
}

async fn titles(db: &DbHandle) -> Vec<String> {
    let rows = db
        .query(
            &format!("SELECT title FROM {TABLE} ORDER BY title"),
            HashMap::new(),
        )
        .await
        .expect("title query");
    rows.iter()
        .map(|r| match r.get("title") {
            Some(Value::String(s)) => s.clone(),
            other => panic!("title column: unexpected value {other:?}"),
        })
        .collect()
}

async fn wait_for_count(db: &DbHandle, want: i64, timeout: Duration) -> i64 {
    let start = Instant::now();
    loop {
        let c = count_rows(db, TABLE).await;
        if c >= want || start.elapsed() > timeout {
            return c;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

// ── Happy / well-formed tool sync (#1) ────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn happy_tool_sync_populates_cache() {
    let db = setup_db().await;
    let integration = connected(
        connect("tool.yaml", MOCK_BIN, Some("happy"), &db)
            .await
            .expect("connect happy"),
    );
    integration.sync_engine.sync_all().await.expect("sync_all");
    assert_eq!(count_rows(&db, TABLE).await, 3, "3 items expected");
    assert_eq!(titles(&db).await, vec!["Item 1", "Item 2", "Item 3"]);
}

// ── Slow / high-latency response (#2) ─────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn slow_response_is_tolerated() {
    let db = setup_db().await;
    let integration = connected(
        connect("tool.yaml", MOCK_BIN, Some("slow"), &db)
            .await
            .expect("connect slow"),
    );
    let start = Instant::now();
    integration.sync_engine.sync_all().await.expect("sync_all");
    assert!(
        start.elapsed() >= Duration::from_millis(250),
        "server-injected latency should be observed"
    );
    assert_eq!(count_rows(&db, TABLE).await, 3);
}

// ── Large result set (#3) ─────────────────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn large_result_set_syncs_fully() {
    let db = setup_db().await;
    let integration = connected(
        connect("tool.yaml", MOCK_BIN, Some("large"), &db)
            .await
            .expect("connect large"),
    );
    integration.sync_engine.sync_all().await.expect("sync_all");
    assert_eq!(count_rows(&db, TABLE).await, 1500, "all 1500 rows synced");
}

// ── Cursor pagination (#4) ────────────────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn cursor_pagination_accumulates() {
    let db = setup_db().await;
    let integration = connected(
        connect("tool_cursor.yaml", MOCK_BIN, Some("paginated"), &db)
            .await
            .expect("connect paginated"),
    );
    // 5 items, page size 2. Each sync_all advances one page (incremental
    // append). Three pages drain all five; a fourth would be an empty page.
    let engine = &integration.sync_engine;
    engine.sync_all().await.expect("page 1");
    assert_eq!(count_rows(&db, TABLE).await, 2, "after page 1");
    engine.sync_all().await.expect("page 2");
    assert_eq!(count_rows(&db, TABLE).await, 4, "after page 2");
    engine.sync_all().await.expect("page 3");
    assert_eq!(count_rows(&db, TABLE).await, 5, "after page 3 all drained");
    assert_eq!(
        titles(&db).await,
        vec!["Item 1", "Item 2", "Item 3", "Item 4", "Item 5"]
    );
}

// ── Malformed (non-JSON) payload (#5) ─────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn malformed_json_fails_loud() {
    let db = setup_db().await;
    let integration = connected(
        connect("tool.yaml", MOCK_BIN, Some("malformed_json"), &db)
            .await
            .expect("connect malformed"),
    );
    let err = integration
        .sync_engine
        .sync_all()
        .await
        .expect_err("malformed payload must fail loud, not empty-cache");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("JSON") || msg.contains("json") || msg.contains("parse"),
        "error should name the parse failure, got: {msg}"
    );
    assert_eq!(
        count_rows(&db, TABLE).await,
        0,
        "no rows written on failure"
    );
}

// ── Schema-violating payload / missing extract field (#6) ─────────
#[tokio::test(flavor = "multi_thread")]
async fn missing_extract_field_fails_loud() {
    let db = setup_db().await;
    let integration = connected(
        connect("tool.yaml", MOCK_BIN, Some("missing_field"), &db)
            .await
            .expect("connect missing_field"),
    );
    let err = integration
        .sync_engine
        .sync_all()
        .await
        .expect_err("missing 'items' field must fail loud");
    assert!(
        format!("{err:#}").contains("items"),
        "error should name the missing field"
    );
    assert_eq!(count_rows(&db, TABLE).await, 0);
}

// ── Dual content block: JSON + trailing prose (#7) ────────────────
#[tokio::test(flavor = "multi_thread")]
async fn dual_text_block_still_parses() {
    let db = setup_db().await;
    let integration = connected(
        connect("tool.yaml", MOCK_BIN, Some("dual_text_block"), &db)
            .await
            .expect("connect dual_text_block"),
    );
    integration.sync_engine.sync_all().await.expect("sync_all");
    assert_eq!(
        count_rows(&db, TABLE).await,
        2,
        "JSON block parsed despite trailing prose block"
    );
}

// ── Tool-level error result (#8) ──────────────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn tool_error_result_fails_loud() {
    let db = setup_db().await;
    let integration = connected(
        connect("tool.yaml", MOCK_BIN, Some("tool_error"), &db)
            .await
            .expect("connect tool_error"),
    );
    let err = integration
        .sync_engine
        .sync_all()
        .await
        .expect_err("isError:true must fail loud");
    assert!(
        format!("{err:#}").contains("rate limit exceeded"),
        "error should surface the tool's own isError message"
    );
    assert_eq!(count_rows(&db, TABLE).await, 0);
}

// ── Stateful resource changing between polls (#9) ─────────────────
#[tokio::test(flavor = "multi_thread")]
async fn stateful_resource_between_polls() {
    let db = setup_db().await;
    let integration = connected(
        connect("resource.yaml", MOCK_BIN, Some("stateful"), &db)
            .await
            .expect("connect stateful"),
    );
    let engine = &integration.sync_engine;
    engine.sync_all().await.expect("poll 1");
    assert_eq!(count_rows(&db, TABLE).await, 1, "first poll: 1 item");
    engine.sync_all().await.expect("poll 2");
    assert_eq!(
        count_rows(&db, TABLE).await,
        2,
        "second poll reflects server-side drift"
    );
}

// ── resources/subscribe + push notification (#10) ─────────────────
#[tokio::test(flavor = "multi_thread")]
async fn subscribe_push_updates_cache() {
    let db = setup_db().await;
    let integration = connected(
        connect("resource.yaml", MOCK_BIN, Some("subscribe_push"), &db)
            .await
            .expect("connect subscribe_push"),
    );
    // The server advertised resources.subscribe; build_mcp_integration
    // subscribed, and the server pushes an `updated` notification ~100ms later
    // after growing the resource to 2 items. Kick an initial sync, then wait
    // for the pushed update to propagate through the background sync loop.
    integration.request_initial_sync().expect("initial sync");
    let got = wait_for_count(&db, 2, Duration::from_secs(5)).await;
    assert_eq!(got, 2, "pushed resource update must reach the cache");
}

// ── Broken initialization handshake (#11) ─────────────────────────
#[tokio::test(flavor = "multi_thread")]
async fn handshake_error_fails_loud() {
    let db = setup_db().await;
    // The raw mock refuses `initialize` with a JSON-RPC error; the connector
    // must surface a hard error, never hang or silently connect.
    let result = tokio::time::timeout(
        Duration::from_secs(20),
        connect("tool.yaml", RAW_BIN, None, &db),
    )
    .await
    .expect("connect must not hang on a broken handshake");
    assert!(
        result.is_err(),
        "a server that refuses initialize must fail loud, got Ok"
    );
}

//! End-to-end test of the `rest` transport against a LOCAL mock HTTP server
//! (no network). Proves that a `transport: rest` sidecar drives the SAME
//! connector read path (`SyncStrategy::fetch_records` over `McpCallSurface`) as
//! the MCP transports, producing Layer-1 replica records from a plain JSON API.
//!
//! The mock is a tiny hand-rolled hyper-free HTTP/1.1 server on a `TcpListener`
//! so the test pulls in no new dependency.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering::SeqCst;

use async_trait::async_trait;
use holon_api::Change;
use holon_api::DynamicEntity;
use holon_api::EntityUri;
use holon_api::StreamPosition;
use holon_api::SyncTokenUpdate;
use holon_api::Value;
use holon_core::DataSource;
use holon_core::EntityCache;
use holon_core::SyncTokenStore;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpSidecar;
use holon_mcp_client::McpSyncEngine;
use holon_mcp_client::McpTransport;
use holon_mcp_client::SyncEvent;
use holon_mcp_client::SyncGate;
use holon_mcp_client::SyncLoopTuning;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::rest_transport::RestCallSurface;
use holon_mcp_client::spawn_sync_event_loop;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// These fixtures declare no credential files, so the root only has to exist;
/// the confinement it enforces is exercised in `credential_confinement.rs`.
fn test_credential_root() -> holon_mcp_client::CredentialRoot {
    holon_mcp_client::CredentialRoot::new("/tmp/holon-rest-transport-mock-config")
}

// ---------------------------------------------------------------------------
// Minimal mock HTTP server
// ---------------------------------------------------------------------------

/// A running mock server: base URL plus the raw request heads it received.
struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

const ATOM_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Mock Atom</title>
  <entry>
    <id>urn:uuid:a1</id>
    <title>Atom one</title>
    <updated>2026-07-12T10:00:00Z</updated>
    <author><name>Ada</name></author>
    <link rel="alternate" href="https://example.com/a1"/>
    <content>Body A</content>
  </entry>
  <entry>
    <id>urn:uuid:a2</id>
    <title>Atom two</title>
    <updated>2026-07-11T09:00:00Z</updated>
    <link href="https://example.com/a2"/>
    <summary>Summary B</summary>
  </entry>
</feed>"#;

const RSS_BODY: &str = r#"<?xml version="1.0"?>
<rss version="2.0">
  <channel>
    <title>Mock RSS</title>
    <item>
      <guid>r-1</guid>
      <title>RSS one</title>
      <pubDate>Sat, 12 Jul 2026 10:00:00 GMT</pubDate>
      <link>https://example.com/r1</link>
      <description>RSS body one</description>
    </item>
  </channel>
</rss>"#;

/// Route a request path to a canned body (JSON or XML). Returns the body text.
fn route(path: &str) -> String {
    if path.starts_with("/posts") {
        // Bare array — the real JSONPlaceholder shape; exercises `result_key`.
        serde_json::json!([
            {"userId": 1, "id": 1, "title": "first", "body": "hello"},
            {"userId": 2, "id": 2, "title": "second", "body": "world"},
        ])
        .to_string()
    } else if path.contains("/users/") {
        serde_json::json!([{"userId": 7, "id": 99, "title": "scoped", "body": "b"}]).to_string()
    } else if path.starts_with("/feed.atom") {
        ATOM_BODY.to_string()
    } else if path.starts_with("/feed.rss") {
        RSS_BODY.to_string()
    } else {
        serde_json::json!([]).to_string()
    }
}

async fn start_mock() -> MockServer {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let requests = Arc::new(Mutex::new(Vec::new()));
    let requests_bg = requests.clone();

    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let requests_bg = requests_bg.clone();
            tokio::spawn(async move {
                // Read the request head (until CRLFCRLF). GET has no body.
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let head = String::from_utf8_lossy(&buf).to_string();
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split_whitespace().nth(1))
                    .unwrap_or("/")
                    .to_string();
                requests_bg.lock().unwrap().push(head);

                let body = route(&path);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                     {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    MockServer {
        base_url: format!("http://{addr}"),
        requests,
    }
}

// ---------------------------------------------------------------------------
// Test token store (ToolSync without a cursor never touches it)
// ---------------------------------------------------------------------------

struct NoopTokenStore;

#[async_trait]
impl SyncTokenStore for NoopTokenStore {
    async fn load_token(&self, _: &str) -> holon_core::Result<Option<StreamPosition>> {
        Ok(None)
    }
    async fn save_token(&self, _: &str, _: StreamPosition) -> holon_core::Result<()> {
        Ok(())
    }
    async fn clear_all_tokens(&self) -> holon_core::Result<()> {
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

fn surface_from(
    yaml: &str,
    entity: &str,
    lookup: &holon_mcp_client::integration_config::VarLookup<'_>,
) -> RestCallSurface {
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml).expect("sidecar parses");
    let mcp_cfg = cfg
        .into_mcp_config_with(entity.to_string(), lookup, &test_credential_root())
        .expect("into_mcp_config");
    match mcp_cfg.transport {
        McpTransport::Rest { manual, .. } => RestCallSurface::new(manual),
        other => panic!("expected rest transport, got {other:?}"),
    }
}

#[tokio::test]
async fn rest_transport_fetches_records_via_shared_sync_path() {
    let mock = start_mock().await;
    let yaml = format!(
        r#"
transport:
  rest:
    base_url: {base}
    calls:
      list-posts:
        method: GET
        path: /posts
        result_key: posts
entities:
  jp_posts:
    id_column: id
    schema:
      - {{ name: id, sql_type: INTEGER, primary_key: true }}
      - {{ name: userId, sql_type: INTEGER }}
      - {{ name: title, sql_type: TEXT }}
      - {{ name: body, sql_type: TEXT }}
    sync:
      list_tool: list-posts
      extract_path: posts
tools: {{}}
"#,
        base = mock.base_url
    );

    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml).expect("parse");
    let strategy = cfg.entities["jp_posts"]
        .sync
        .as_ref()
        .expect("sync present")
        .into_strategy()
        .expect("strategy");

    let surface = surface_from(&yaml, "jsonplaceholder", &|_| None);
    let store = NoopTokenStore;

    let result = strategy
        .fetch_records(&surface, &store, "jsonplaceholder.jp_posts")
        .await
        .expect("fetch_records via rest transport");

    assert_eq!(result.records.len(), 2, "expected two posts");
    let ids: Vec<i64> = result
        .records
        .iter()
        .map(|r| r.get("id").and_then(|v| v.as_i64()).expect("id"))
        .collect();
    assert_eq!(ids, vec![1, 2]);
    assert_eq!(
        result.records[0].get("title").and_then(|v| v.as_str()),
        Some("first")
    );
    assert!(result.new_cursor.is_none());

    // The mock actually received a GET /posts.
    let reqs = mock.requests.lock().unwrap();
    assert!(
        reqs.iter().any(|r| r.starts_with("GET /posts")),
        "mock never saw GET /posts; saw {reqs:?}"
    );
}

async fn fetch_feed(
    format: &str,
    path: &str,
    base: &str,
) -> Vec<serde_json::Map<String, serde_json::Value>> {
    let yaml = format!(
        r#"
transport:
  rest:
    base_url: {base}
    calls:
      list-feed:
        method: GET
        path: {path}
        format: {format}
        result_key: entries
entities:
  feed_items:
    id_column: id
    schema:
      - {{ name: id, sql_type: TEXT, primary_key: true }}
      - {{ name: title, sql_type: TEXT }}
      - {{ name: updated, sql_type: TEXT }}
      - {{ name: author, sql_type: TEXT }}
      - {{ name: link, sql_type: TEXT }}
      - {{ name: content, sql_type: TEXT }}
    sync:
      list_tool: list-feed
      extract_path: entries
tools: {{}}
"#
    );
    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml).expect("parse");
    let strategy = cfg.entities["feed_items"]
        .sync
        .as_ref()
        .expect("sync")
        .into_strategy()
        .expect("strategy");
    let surface = surface_from(&yaml, "feed", &|_| None);
    strategy
        .fetch_records(&surface, &NoopTokenStore, "feed.feed_items")
        .await
        .expect("fetch feed")
        .records
}

#[tokio::test]
async fn rest_transport_decodes_atom_feed_via_shared_sync_path() {
    let mock = start_mock().await;
    let records = fetch_feed("atom", "/feed.atom", &mock.base_url).await;
    assert_eq!(records.len(), 2, "expected two atom entries");
    assert_eq!(
        records[0].get("id").and_then(|v| v.as_str()),
        Some("urn:uuid:a1")
    );
    assert_eq!(
        records[0].get("title").and_then(|v| v.as_str()),
        Some("Atom one")
    );
    assert_eq!(
        records[0].get("author").and_then(|v| v.as_str()),
        Some("Ada")
    );
    assert_eq!(
        records[0].get("link").and_then(|v| v.as_str()),
        Some("https://example.com/a1")
    );
    assert_eq!(
        records[0].get("content").and_then(|v| v.as_str()),
        Some("Body A")
    );
    // Second entry falls back to <summary>.
    assert_eq!(
        records[1].get("content").and_then(|v| v.as_str()),
        Some("Summary B")
    );
}

#[tokio::test]
async fn rest_transport_decodes_rss_feed_via_shared_sync_path() {
    let mock = start_mock().await;
    let records = fetch_feed("rss", "/feed.rss", &mock.base_url).await;
    assert_eq!(records.len(), 1, "expected one rss item");
    assert_eq!(records[0].get("id").and_then(|v| v.as_str()), Some("r-1"));
    assert_eq!(
        records[0].get("title").and_then(|v| v.as_str()),
        Some("RSS one")
    );
    assert_eq!(
        records[0].get("link").and_then(|v| v.as_str()),
        Some("https://example.com/r1")
    );
    assert_eq!(
        records[0].get("content").and_then(|v| v.as_str()),
        Some("RSS body one")
    );
}

#[tokio::test]
async fn rest_transport_sends_auth_header_and_fills_path_placeholder() {
    let mock = start_mock().await;
    // Secret referenced by env name only — never inlined.
    // SAFETY: single-threaded test setup before any concurrent env access.
    unsafe {
        std::env::set_var("REST_TEST_TOKEN", "s3cr3t");
    }
    let yaml = format!(
        r#"
transport:
  rest:
    base_url: {base}
    auth:
      header: Authorization
      value: "Bearer ${{REST_TEST_TOKEN}}"
    calls:
      user-posts:
        method: GET
        path: /users/{{userId}}/posts
entities: {{}}
tools: {{}}
"#,
        base = mock.base_url
    );

    let surface = surface_from(
        &yaml,
        "scoped",
        &holon_mcp_client::integration_config::env_var_lookup,
    );

    // Drive call_tool directly through the McpCallSurface seam.
    use holon_mcp_client::mcp_call_surface::McpCallSurface;
    use rmcp::model::CallToolRequestParam;
    let mut args = serde_json::Map::new();
    args.insert("userId".into(), serde_json::json!("7"));
    let out = surface
        .call_tool(CallToolRequestParam {
            name: "user-posts".into(),
            arguments: Some(args),
        })
        .await
        .expect("call_tool");
    let structured = out.structured_content.expect("structured content");
    // No result_key -> body passed through as a bare array.
    assert!(structured.is_array());

    let reqs = mock.requests.lock().unwrap();
    let head = reqs
        .iter()
        .find(|r| r.starts_with("GET /users/7/posts"))
        .unwrap_or_else(|| panic!("path placeholder not filled; saw {reqs:?}"));
    assert!(
        head.contains("authorization: Bearer s3cr3t")
            || head.contains("Authorization: Bearer s3cr3t"),
        "auth header missing; head was:\n{head}"
    );
}

#[tokio::test]
async fn rest_transport_read_resource_fails_loud() {
    let mock = start_mock().await;
    let yaml = format!(
        "transport:\n  rest:\n    base_url: {base}\n    calls:\n      x:\n        method: \
             GET\n        path: /posts\nentities: {{}}\ntools: {{}}\n",
        base = mock.base_url
    );
    let surface = surface_from(&yaml, "p", &|_| None);
    use holon_mcp_client::mcp_call_surface::McpCallSurface;
    use rmcp::model::ReadResourceRequestParam;
    let err = surface
        .read_resource(ReadResourceRequestParam {
            uri: "foo://bar".into(),
        })
        .await
        .expect_err("read_resource must fail on rest transport");
    assert!(
        err.to_string().contains("does not support MCP resources"),
        "unexpected error: {err}"
    );
}

#[test]
fn shipped_jsonplaceholder_sidecar_parses_as_rest() {
    // The committed example file is a valid rest-transport sidecar.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/integrations/jsonplaceholder.yaml"
    );
    let yaml = std::fs::read_to_string(path).expect("read jsonplaceholder.yaml");
    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml).expect("example parses");
    let mcp_cfg = cfg
        .into_mcp_config("jsonplaceholder".to_string(), &test_credential_root())
        .expect("into_mcp_config");
    match mcp_cfg.transport {
        McpTransport::Rest {
            manual,
            poll_interval,
        } => {
            assert_eq!(manual.base_url, "https://jsonplaceholder.typicode.com");
            assert!(manual.calls.contains_key("list-posts"));
            // Shipped example sets no poll_interval → resolves to the 300s default.
            assert!(poll_interval.is_none());
        }
        other => panic!("expected rest transport, got {other:?}"),
    }
}

#[test]
fn unknown_transport_field_is_rejected() {
    // deny_unknown_fields: a typo'd transport key fails loud at parse.
    let yaml = "transport:\n  htttp:\n    uri: x\nentities: {}\ntools: {}\n";
    let err = serde_yaml::from_str::<IntegrationFileConfig>(yaml).unwrap_err();
    assert!(err.to_string().contains("htttp") || err.to_string().contains("unknown field"));
}

#[test]
fn rest_poll_interval_parses() {
    // The transport-wide default cadence parses (humantime string) and threads
    // onto the resolved transport variant.
    let yaml = r#"
transport:
  rest:
    base_url: http://x
    poll_interval: 90s
    calls:
      x:
        method: GET
        path: /p
entities: {}
tools: {}
"#;
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml).expect("parse");
    let mcp_cfg = cfg
        .into_mcp_config("p".to_string(), &test_credential_root())
        .expect("into_mcp_config");
    match mcp_cfg.transport {
        McpTransport::Rest { poll_interval, .. } => {
            assert_eq!(
                poll_interval.expect("poll_interval set").duration(),
                std::time::Duration::from_secs(90)
            );
        }
        other => panic!("expected rest transport, got {other:?}"),
    }
}

#[test]
fn rest_unknown_field_under_transport_rest_is_rejected() {
    // deny_unknown_fields on RestTransport: a typo under transport.rest fails
    // loud at parse rather than being silently ignored.
    let yaml = "transport:\n  rest:\n    base_url: http://x\n    poll_intervall: 90s\n    calls: \
                     {}\nentities: {}\ntools: {}\n";
    let err = serde_yaml::from_str::<IntegrationFileConfig>(yaml).unwrap_err();
    assert!(
        err.to_string().contains("poll_intervall") || err.to_string().contains("unknown field"),
        "unexpected error: {err}"
    );
}

// ---------------------------------------------------------------------------
// Controllable mock + counting cache: end-to-end poll-runner drive
// ---------------------------------------------------------------------------

/// Mutable server state so a test can swap the response body and force a 500 to
/// prove the poll loop's failure survival and mirror-reuse (no re-apply when
/// nothing changed).
struct MockState {
    body: String,
    force_500: bool,
}

struct ControlledMock {
    base_url: String,
    state: Arc<Mutex<MockState>>,
}

impl ControlledMock {
    fn set_body(&self, body: impl Into<String>) {
        self.state.lock().unwrap().body = body.into();
    }
    fn set_force_500(&self, on: bool) {
        self.state.lock().unwrap().force_500 = on;
    }
}

async fn start_controlled_mock(initial_body: impl Into<String>) -> ControlledMock {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let state = Arc::new(Mutex::new(MockState {
        body: initial_body.into(),
        force_500: false,
    }));
    let state_bg = state.clone();

    tokio::spawn(async move {
        loop {
            let (mut socket, _) = match listener.accept().await {
                Ok(pair) => pair,
                Err(_) => break,
            };
            let state_bg = state_bg.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                let (body, force_500) = {
                    let s = state_bg.lock().unwrap();
                    (s.body.clone(), s.force_500)
                };
                let response = if force_500 {
                    "HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: \
                     close\r\n\r\n"
                        .to_string()
                } else {
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                         {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    )
                };
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    ControlledMock {
        base_url: format!("http://{addr}"),
        state,
    }
}

/// An in-memory `EntityCache` that also counts `apply_batch` invocations, so a
/// test can prove (a) the initial sync applies once, (b) an unchanged poll does
/// NOT re-apply (mirror-reuse), and (c) a changed poll applies exactly the
/// delta. Rows are keyed by a canonicalized entity URI so create/update/delete
/// stay consistent with the engine's mirror.
struct CountingCache {
    rows: Mutex<HashMap<String, DynamicEntity>>,
    apply_batch_calls: AtomicUsize,
    id_col: String,
}

impl CountingCache {
    fn new(id_col: &str) -> Self {
        Self {
            rows: Mutex::new(HashMap::new()),
            apply_batch_calls: AtomicUsize::new(0),
            id_col: id_col.to_string(),
        }
    }

    fn apply_batch_calls(&self) -> usize {
        self.apply_batch_calls.load(SeqCst)
    }

    fn len(&self) -> usize {
        self.rows.lock().unwrap().len()
    }

    fn title_of(&self, canonical_id: &str) -> Option<String> {
        self.rows
            .lock()
            .unwrap()
            .get(canonical_id)
            .and_then(|e| e.get("title"))
            .and_then(|v| v.as_string().map(String::from))
    }

    /// Canonical key from a raw (already scheme-prefixed) id string.
    fn key_from_raw(raw: &str) -> String {
        // ALLOW(entity_uri_from_raw): test cache keying mirrors the sync engine
        EntityUri::from_raw(raw).to_string()
    }

    fn key_of_entity(&self, e: &DynamicEntity) -> String {
        let raw = match e.get(&self.id_col) {
            // The sync engine always writes the id column as a scheme-prefixed
            // string (see `record_to_entity`), so this is the only live arm.
            Some(Value::String(s)) => s.clone(),
            other => panic!(
                "entity id column '{}' is not a prefixed string: {other:?}",
                self.id_col
            ),
        };
        Self::key_from_raw(&raw)
    }
}

#[async_trait]
impl DataSource<DynamicEntity> for CountingCache {
    async fn get_all(&self) -> holon_core::Result<Vec<DynamicEntity>> {
        Ok(self.rows.lock().unwrap().values().cloned().collect())
    }
    async fn get_by_id(&self, id: &str) -> holon_core::Result<Option<DynamicEntity>> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .get(&Self::key_from_raw(id))
            .cloned())
    }
}

#[async_trait]
impl EntityCache<DynamicEntity> for CountingCache {
    async fn get_all_ids(&self) -> holon_core::Result<Vec<EntityUri>> {
        Ok(self
            .rows
            .lock()
            .unwrap()
            .values()
            .map(|e| self.key_of_entity(e))
            // ALLOW(entity_uri_from_raw): canonical key round-trips to a URI
            .map(|k| EntityUri::from_raw(&k))
            .collect())
    }

    async fn apply_batch(
        &self,
        changes: &[Change<DynamicEntity>],
        // ALLOW(unused_param): trait-required by EntityCache; test cache stores no token.
        _sync_token: Option<&SyncTokenUpdate>,
    ) -> holon_core::Result<()> {
        self.apply_batch_calls.fetch_add(1, SeqCst);
        let mut rows = self.rows.lock().unwrap();
        for change in changes {
            match change {
                Change::Created { data, .. } => {
                    rows.insert(self.key_of_entity(data), data.clone());
                }
                Change::Updated { id, data, .. } => {
                    rows.insert(Self::key_from_raw(id), data.clone());
                }
                Change::Deleted { id, .. } => {
                    rows.remove(&Self::key_from_raw(id));
                }
                Change::FieldsChanged { .. } => {
                    unreachable!("mcp sync never emits FieldsChanged")
                }
            }
        }
        Ok(())
    }

    async fn upsert(&self, item: &DynamicEntity) -> holon_core::Result<()> {
        self.rows
            .lock()
            .unwrap()
            .insert(self.key_of_entity(item), item.clone());
        Ok(())
    }

    async fn delete(&self, id: &str) -> holon_core::Result<()> {
        self.rows.lock().unwrap().remove(&Self::key_from_raw(id));
        Ok(())
    }
}

fn posts_body(first_title: &str) -> String {
    serde_json::json!([
        {"userId": 1, "id": 1, "title": first_title, "body": "hello"},
        {"userId": 2, "id": 2, "title": "second", "body": "world"},
    ])
    .to_string()
}

fn poll_runner_yaml(base: &str) -> String {
    format!(
        r#"
transport:
  rest:
    base_url: {base}
    calls:
      list-posts:
        method: GET
        path: /posts
        result_key: posts
entities:
  jp_posts:
    id_column: id
    schema:
      - {{ name: id, sql_type: INTEGER, primary_key: true }}
      - {{ name: userId, sql_type: INTEGER }}
      - {{ name: title, sql_type: TEXT }}
      - {{ name: body, sql_type: TEXT }}
    sync:
      list_tool: list-posts
      extract_path: posts
tools: {{}}
"#
    )
}

/// Poll every 10ms until `cond` holds or the deadline passes; returns whether
/// it held. Real time (the RestCallSurface does real socket IO).
async fn wait_until<F: Fn() -> bool>(cond: F) -> bool {
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    while tokio::time::Instant::now() < deadline {
        if cond() {
            return true;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    cond()
}

#[tokio::test]
async fn rest_poll_runner_syncs_reuses_mirror_and_survives_failure() {
    let mock = start_controlled_mock(posts_body("first")).await;
    let yaml = poll_runner_yaml(&mock.base_url);

    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml).expect("parse cfg");
    let strategy = cfg.entities["jp_posts"]
        .sync
        .as_ref()
        .expect("sync present")
        .into_strategy()
        .expect("strategy");
    let sidecar: McpSidecar = serde_yaml::from_str(&yaml).expect("parse sidecar");

    let manual = match cfg
        .into_mcp_config_with("jp".to_string(), &|_| None, &test_credential_root())
        .expect("into_mcp_config")
        .transport
    {
        McpTransport::Rest { manual, .. } => manual,
        other => panic!("expected rest transport, got {other:?}"),
    };
    let surface: Arc<dyn McpCallSurface> = Arc::new(RestCallSurface::new(manual));

    let cache = Arc::new(CountingCache::new("id"));
    let mut caches: HashMap<String, Arc<dyn EntityCache<DynamicEntity>>> = HashMap::new();
    caches.insert("jp_posts".to_string(), cache.clone());
    let mut strategies: HashMap<String, Box<dyn holon_mcp_client::SyncStrategy>> = HashMap::new();
    strategies.insert("jp_posts".to_string(), strategy);

    let engine = Arc::new(McpSyncEngine::new(
        surface,
        None,
        strategies,
        caches,
        Arc::new(NoopTokenStore),
        "jp".to_string(),
        sidecar.clone(),
        Vec::new(),
        None,
    ));

    let (tx, rx) = tokio::sync::mpsc::unbounded_channel::<SyncEvent>();
    let _loop = spawn_sync_event_loop(
        rx,
        engine.clone(),
        SyncGate::opened(),
        SyncLoopTuning::test(),
    );

    // Canonical ids for post 1 / 2 under the "jp_posts" scheme.
    let scheme = sidecar.prefixed_name("jp_posts").as_str().to_string();
    let id1 = CountingCache::key_from_raw(&format!("{scheme}:1"));

    // T1: initial full sync — 2 posts land via exactly one apply_batch.
    tx.send(SyncEvent::SyncAll).unwrap();
    assert!(
        wait_until(|| cache.apply_batch_calls() == 1 && cache.len() == 2).await,
        "initial sync should apply once and land 2 rows (applies={}, len={})",
        cache.apply_batch_calls(),
        cache.len()
    );
    assert_eq!(cache.title_of(&id1).as_deref(), Some("first"));

    // T2: unchanged body → the mirror diff is empty → NO re-apply.
    tx.send(SyncEvent::PollTick("jp_posts".into())).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(
        cache.apply_batch_calls(),
        1,
        "unchanged poll must not re-apply (mirror reuse)"
    );

    // T2b: body changed → exactly the delta (one Updated) applies.
    mock.set_body(posts_body("first-EDITED"));
    tx.send(SyncEvent::PollTick("jp_posts".into())).unwrap();
    assert!(
        wait_until(|| cache.apply_batch_calls() == 2).await,
        "changed poll should apply the delta (applies={})",
        cache.apply_batch_calls()
    );
    assert_eq!(cache.title_of(&id1).as_deref(), Some("first-EDITED"));
    assert_eq!(cache.len(), 2, "an update must not change row count");

    // T3: forced 500 → fetch fails, applies unchanged, loop SURVIVES.
    mock.set_force_500(true);
    tx.send(SyncEvent::PollTick("jp_posts".into())).unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(400)).await;
    assert_eq!(
        cache.apply_batch_calls(),
        2,
        "a 500 must not apply anything"
    );

    // Restore 200 with a fresh delta → the next tick lands (loop alive).
    mock.set_force_500(false);
    mock.set_body(posts_body("first-RECOVERED"));
    tx.send(SyncEvent::PollTick("jp_posts".into())).unwrap();
    assert!(
        wait_until(|| cache.apply_batch_calls() == 3).await,
        "loop must survive the 500 and apply the next delta (applies={})",
        cache.apply_batch_calls()
    );
    assert_eq!(cache.title_of(&id1).as_deref(), Some("first-RECOVERED"));
}

//! End-to-end test of the `rest` transport against a LOCAL mock HTTP server
//! (no network). Proves that a `transport: rest` sidecar drives the SAME
//! connector read path (`SyncStrategy::fetch_records` over `McpCallSurface`) as
//! the MCP transports, producing Layer-1 replica records from a plain JSON API.
//!
//! The mock is a tiny hand-rolled hyper-free HTTP/1.1 server on a `TcpListener`
//! so the test pulls in no new dependency.

use std::sync::Arc;
use std::sync::Mutex;

use async_trait::async_trait;
use holon_api::StreamPosition;
use holon_core::SyncTokenStore;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::rest_transport::RestCallSurface;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Minimal mock HTTP server
// ---------------------------------------------------------------------------

/// A running mock server: base URL plus the raw request heads it received.
struct MockServer {
    base_url: String,
    requests: Arc<Mutex<Vec<String>>>,
}

/// Route a request path to a canned JSON body. Returns the body text.
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
    lookup: &dyn Fn(&str) -> Option<String>,
) -> RestCallSurface {
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml).expect("sidecar parses");
    let mcp_cfg = cfg
        .into_mcp_config_with(entity.to_string(), lookup)
        .expect("into_mcp_config");
    match mcp_cfg.transport {
        McpTransport::Rest(manual) => RestCallSurface::new(manual),
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

#[tokio::test]
async fn rest_transport_rejects_non_get_method() {
    let mock = start_mock().await;
    let yaml = format!(
        "transport:\n  rest:\n    base_url: {base}\n    calls:\n      mutate:\n        method: \
         POST\n        path: /posts\nentities: {{}}\ntools: {{}}\n",
        base = mock.base_url
    );
    let surface = surface_from(&yaml, "p", &|_| None);
    use holon_mcp_client::mcp_call_surface::McpCallSurface;
    use rmcp::model::CallToolRequestParam;
    let err = surface
        .call_tool(CallToolRequestParam {
            name: "mutate".into(),
            arguments: None,
        })
        .await
        .expect_err("POST must be rejected");
    assert!(
        err.to_string().contains("only GET is supported"),
        "unexpected error: {err}"
    );
}

#[test]
fn shipped_jsonplaceholder_sidecar_parses_as_rest() {
    // The committed example file is a valid rest-transport sidecar.
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/integrations/jsonplaceholder.yaml"
    );
    let yaml = std::fs::read_to_string(path).expect("read jsonplaceholder.yaml");
    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml).expect("example parses");
    let mcp_cfg = cfg
        .into_mcp_config("jsonplaceholder".to_string())
        .expect("into_mcp_config");
    match mcp_cfg.transport {
        McpTransport::Rest(m) => {
            assert_eq!(m.base_url, "https://jsonplaceholder.typicode.com");
            assert!(m.calls.contains_key("list-posts"));
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

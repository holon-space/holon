//! End-to-end tests of the `rest` transport's generic OAuth2 auth arm and
//! response-token pagination, against a LOCAL mock HTTP server (no network).
//!
//! Exercises the FULL path — YAML → `into_mcp_config_with` (which reads the
//! 0600 refresh-token file and builds the token provider) → `RestCallSurface`
//! → `call_tool` — proving: the refresh-token grant mints an access token and
//! attaches it as `Authorization: Bearer`; a 401 triggers exactly one
//! token-refresh + retry; pagination follows `nextPageToken` and fails loud at
//! `max_pages`; and NO token/secret material ever appears in an error string.

use std::sync::Arc;
use std::sync::Mutex;

use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::rest_transport::RestCallSurface;
use rmcp::model::CallToolRequestParam;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

// ---------------------------------------------------------------------------
// Controllable mock: a token endpoint + protected/paged GET endpoints.
// ---------------------------------------------------------------------------

#[derive(Default)]
struct MockState {
    /// Request heads (method line + headers) the server received.
    requests: Vec<String>,
    /// How many times POST /token was hit.
    token_posts: u32,
    /// Access-token counter; each refresh mints `access-<n>`.
    mint_counter: u32,
    /// When true, the next protected GET returns 401 once (simulates a token
    /// the server rejected before the client thought it expired).
    protected_401_once: bool,
    /// When true, POST /token returns an OAuth error (with a token-shaped field
    /// planted to prove redaction never echoes the body).
    token_endpoint_fails: bool,
    /// When true, /paged always returns a nextPageToken (to exercise the
    /// bound).
    paged_infinite: bool,
}

struct Mock {
    base_url: String,
    state: Arc<Mutex<MockState>>,
}

impl Mock {
    fn with<R>(&self, f: impl FnOnce(&mut MockState) -> R) -> R {
        f(&mut self.state.lock().unwrap())
    }
    fn requests(&self) -> Vec<String> {
        self.state.lock().unwrap().requests.clone()
    }
}

fn header_value(head: &str, name_lower: &str) -> Option<String> {
    head.lines()
        .find(|l| l.to_ascii_lowercase().starts_with(name_lower))
        .and_then(|l| l.split_once(':'))
        .map(|(_, v)| v.trim().to_string())
}

fn respond(status: &str, body: &str) -> String {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: \
         close\r\n\r\n{body}",
        body.len()
    )
}

async fn start_mock() -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let state = Arc::new(Mutex::new(MockState::default()));
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
                let head = String::from_utf8_lossy(&buf).to_string();
                let request_line = head.lines().next().unwrap_or("").to_string();
                let mut parts = request_line.split_whitespace();
                let method = parts.next().unwrap_or("").to_string();
                let target = parts.next().unwrap_or("/").to_string();
                let path = target.split('?').next().unwrap_or("/").to_string();
                let query = target.splitn(2, '?').nth(1).unwrap_or("").to_string();

                let response = {
                    let mut s = state_bg.lock().unwrap();
                    s.requests.push(head.clone());

                    if method == "POST" && path == "/token" {
                        s.token_posts += 1;
                        if s.token_endpoint_fails {
                            // A token-shaped field is planted here on purpose: the
                            // engine must surface only error/error_description.
                            respond(
                                "401 Unauthorized",
                                r#"{"error":"invalid_grant","error_description":"Token has been expired","access_token":"ya29.PLANTED_LEAK"}"#,
                            )
                        } else {
                            s.mint_counter += 1;
                            let body = format!(
                                r#"{{"access_token":"access-{}","expires_in":3600,"token_type":"Bearer"}}"#,
                                s.mint_counter
                            );
                            respond("200 OK", &body)
                        }
                    } else if path == "/thing" {
                        let authed = header_value(&head, "authorization")
                            .is_some_and(|v| v.starts_with("Bearer access-"));
                        if !authed {
                            respond("401 Unauthorized", r#"{"error":"missing bearer"}"#)
                        } else if s.protected_401_once {
                            s.protected_401_once = false;
                            respond("401 Unauthorized", r#"{"error":"stale token"}"#)
                        } else {
                            respond("200 OK", r#"{"things":[{"id":1,"name":"ok"}]}"#)
                        }
                    } else if path == "/paged" {
                        let authed = header_value(&head, "authorization")
                            .is_some_and(|v| v.starts_with("Bearer "));
                        if !authed {
                            respond("401 Unauthorized", r#"{"error":"missing bearer"}"#)
                        } else if s.paged_infinite {
                            respond(
                                "200 OK",
                                r#"{"items":[{"id":9}],"nextPageToken":"always-more"}"#,
                            )
                        } else if query.contains("pageToken=t1") {
                            respond("200 OK", r#"{"items":[{"id":2}]}"#)
                        } else {
                            respond("200 OK", r#"{"items":[{"id":1}],"nextPageToken":"t1"}"#)
                        }
                    } else {
                        respond("404 Not Found", r#"{"error":"no route"}"#)
                    }
                };

                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    Mock {
        base_url: format!("http://{addr}"),
        state,
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A temp refresh-token file at mode 0600 (kept alive by the returned dir).
fn temp_refresh_token(contents: &str) -> (tempfile::TempDir, String) {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("gcal-refresh-token");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    #[cfg(unix)]
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    (dir, path.to_str().unwrap().to_string())
}

fn oauth_yaml(base: &str, refresh_file: &str, call: &str) -> String {
    format!(
        r#"
transport:
  rest:
    base_url: {base}
    auth:
      oauth2:
        token_url: {base}/token
        client_id_env: OAUTH_TEST_CLIENT_ID
        client_secret_env: OAUTH_TEST_CLIENT_SECRET
        refresh_token_file: {refresh_file}
        scopes: [scope.readonly]
    calls:
{call}
entities: {{}}
tools: {{}}
"#
    )
}

const GET_THING_CALL: &str = r#"      get-thing:
        method: GET
        path: /thing
"#;

fn paged_call(max_pages: u32) -> String {
    format!(
        r#"      list-paged:
        method: GET
        path: /paged
        pagination:
          items_path: items
          next_token_path: nextPageToken
          token_param: pageToken
          max_pages: {max_pages}
"#
    )
}

fn lookup(name: &str) -> Option<String> {
    match name {
        "OAUTH_TEST_CLIENT_ID" => Some("client-id-123".to_string()),
        "OAUTH_TEST_CLIENT_SECRET" => Some("client-secret-xyz".to_string()),
        _ => None,
    }
}

fn surface_from(yaml: &str) -> RestCallSurface {
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml).expect("sidecar parses");
    let mcp = cfg
        .into_mcp_config_with("gcal".to_string(), &lookup)
        .expect("into_mcp_config");
    match mcp.transport {
        McpTransport::Rest { manual, .. } => RestCallSurface::new(manual),
        other => panic!("expected rest transport, got {other:?}"),
    }
}

async fn call(surface: &RestCallSurface, name: &str) -> Result<serde_json::Value, String> {
    surface
        .call_tool(CallToolRequestParam {
            name: name.to_string().into(),
            arguments: None,
        })
        .await
        .map(|r| r.structured_content.expect("structured"))
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn oauth2_refreshes_and_attaches_bearer() {
    let mock = start_mock().await;
    let (_dir, rt) = temp_refresh_token("refresh-abc\n");
    let yaml = oauth_yaml(&mock.base_url, &rt, GET_THING_CALL);
    let surface = surface_from(&yaml);

    let out = call(&surface, "get-thing").await.expect("call ok");
    assert_eq!(out["things"][0]["name"], "ok");

    // Exactly one token POST, and the protected GET carried the minted bearer.
    assert_eq!(mock.with(|s| s.token_posts), 1);
    let reqs = mock.requests();
    assert!(
        reqs.iter().any(|r| r.starts_with("POST /token")),
        "no token POST seen: {reqs:?}"
    );
    assert!(
        reqs.iter().any(|r| r.starts_with("GET /thing")
            && r.to_ascii_lowercase()
                .contains("authorization: bearer access-1")),
        "protected GET missing bearer: {reqs:?}"
    );

    // A second call reuses the cached token (no extra refresh).
    call(&surface, "get-thing").await.expect("second call ok");
    assert_eq!(
        mock.with(|s| s.token_posts),
        1,
        "cached token must be reused"
    );
}

#[tokio::test]
async fn oauth2_retries_once_on_401() {
    let mock = start_mock().await;
    mock.with(|s| s.protected_401_once = true);
    let (_dir, rt) = temp_refresh_token("refresh-abc");
    let yaml = oauth_yaml(&mock.base_url, &rt, GET_THING_CALL);
    let surface = surface_from(&yaml);

    let out = call(&surface, "get-thing")
        .await
        .expect("call recovers after 401");
    assert_eq!(out["things"][0]["id"], 1);
    // Two token POSTs: initial mint + the forced refresh on the 401.
    assert_eq!(
        mock.with(|s| s.token_posts),
        2,
        "401 must force exactly one refresh"
    );
}

#[tokio::test]
async fn oauth2_refresh_failure_redacts_all_secrets() {
    let mock = start_mock().await;
    mock.with(|s| s.token_endpoint_fails = true);
    let (_dir, rt) = temp_refresh_token("refresh-SECRET-abc");
    let yaml = oauth_yaml(&mock.base_url, &rt, GET_THING_CALL);
    let surface = surface_from(&yaml);

    let err = call(&surface, "get-thing")
        .await
        .expect_err("token refresh must fail");
    // The safe OAuth error surfaces...
    assert!(
        err.contains("invalid_grant"),
        "actionable error missing: {err}"
    );
    // ...but NO secret material of any kind leaks.
    for needle in [
        "refresh-SECRET-abc", // the refresh token
        "client-secret-xyz",  // the client secret
        "ya29.PLANTED_LEAK",  // the planted access-token-shaped body field
    ] {
        assert!(!err.contains(needle), "error leaked '{needle}': {err}");
    }
}

#[tokio::test]
async fn pagination_follows_next_token_and_concatenates() {
    let mock = start_mock().await;
    let (_dir, rt) = temp_refresh_token("refresh-abc");
    let yaml = oauth_yaml(&mock.base_url, &rt, &paged_call(10));
    let surface = surface_from(&yaml);

    let out = call(&surface, "list-paged").await.expect("paged call ok");
    let items = out["items"].as_array().expect("items array");
    assert_eq!(items.len(), 2, "both pages concatenated: {out}");
    assert_eq!(items[0]["id"], 1);
    assert_eq!(items[1]["id"], 2);
    // The second request carried the continuation token.
    assert!(
        mock.requests()
            .iter()
            .any(|r| r.starts_with("GET /paged") && r.contains("pageToken=t1")),
        "continuation token never sent"
    );
}

#[tokio::test]
async fn pagination_exceeding_max_pages_fails_loud() {
    let mock = start_mock().await;
    mock.with(|s| s.paged_infinite = true);
    let (_dir, rt) = temp_refresh_token("refresh-abc");
    let yaml = oauth_yaml(&mock.base_url, &rt, &paged_call(3));
    let surface = surface_from(&yaml);

    let err = call(&surface, "list-paged")
        .await
        .expect_err("must fail loud at the bound");
    assert!(err.contains("max_pages=3"), "unexpected error: {err}");
    assert!(err.contains("unbounded"), "unexpected error: {err}");
}

// ---------------------------------------------------------------------------
// Shipped gcal.yaml shape (no server)
// ---------------------------------------------------------------------------

#[test]
fn shipped_gcal_sidecar_shape() {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/integrations/gcal.yaml"
    );
    let yaml = std::fs::read_to_string(path).expect("read gcal.yaml");
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(&yaml).expect("gcal.yaml parses as IntegrationFileConfig");

    assert_eq!(cfg.entity_prefix.as_deref(), Some("gcal_"));
    let rest = cfg.transport.rest.as_ref().expect("rest transport");
    assert_eq!(rest.base_url, "https://www.googleapis.com/calendar/v3");

    // OAuth2 auth arm is present and well-formed.
    let auth = rest.auth.as_ref().expect("auth block");
    let oauth2 = auth.oauth2.as_ref().expect("oauth2 arm");
    assert_eq!(oauth2.token_url, "https://oauth2.googleapis.com/token");
    // Credentials are file-sourced (Holon is a GUI app; env vars don't reach it
    // cleanly). The env variant stays documented in the yaml as the alternative.
    assert_eq!(
        oauth2.client_id_file.as_deref(),
        Some("~/.config/holon/gcal-client-id")
    );
    assert_eq!(
        oauth2.client_secret_file.as_deref(),
        Some("~/.config/holon/gcal-client-secret")
    );
    assert!(
        oauth2.client_id_env.is_none() && oauth2.client_secret_env.is_none(),
        "gcal.yaml sources credentials from files, not env"
    );
    assert!(oauth2.refresh_token_file.contains("gcal-refresh-token"));
    assert!(
        auth.header.is_none() && auth.value.is_none(),
        "must not mix static header with oauth2"
    );

    // list-events carries the rolling window + pagination.
    let events = rest.calls.get("list-events").expect("list-events call");
    assert_eq!(
        events.query.get("singleEvents").map(String::as_str),
        Some("true")
    );
    assert_eq!(
        events.query.get("timeMin").map(String::as_str),
        Some("{now-1d}")
    );
    assert_eq!(
        events.query.get("timeMax").map(String::as_str),
        Some("{now+14d}")
    );
    let pg = events.pagination.as_ref().expect("events pagination");
    assert_eq!(pg.next_token_path, "nextPageToken");
    assert_eq!(pg.token_param, "pageToken");
    assert!(pg.max_pages >= 1);

    // Two entities; event uses field projection to flatten start/end/all_day.
    assert!(cfg.entities.contains_key("calendar"));
    let event = cfg.entities.get("event").expect("event entity");
    let sync = event.sync.as_ref().expect("event sync");
    assert_eq!(
        sync.list_params.get("calendar_id").and_then(|v| v.as_str()),
        Some("primary")
    );
    assert!(
        sync.project.contains_key("start"),
        "start projection missing"
    );
    assert!(
        sync.project.contains_key("all_day"),
        "all_day projection missing"
    );

    // Two chained upcoming views.
    let view_names: Vec<&str> = cfg.views.iter().map(|v| v.name.as_str()).collect();
    assert!(
        view_names.contains(&"upcoming_flagged"),
        "views: {view_names:?}"
    );
    assert!(view_names.contains(&"upcoming"), "views: {view_names:?}");
}

// Note: generic structural validation (schema → valid CREATE TABLE DDL, views →
// IVM-valid, sync/projection cross-refs) lives in `sidecar_conformance.rs`,
// which runs against EVERY shipped sidecar — including gcal — so it is not
// duplicated here. This file keeps only gcal's provider-SPECIFIC contract
// (the oauth2 arm + pinned-primary + rolling window in
// `shipped_gcal_sidecar_shape`) plus the engine-mechanism tests above, which
// use synthetic YAML and are provider-agnostic by construction.

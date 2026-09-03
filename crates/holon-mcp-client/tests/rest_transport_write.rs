//! The `rest` transport's WRITE leg against a LOCAL mock HTTP server (no
//! network): a non-GET method, a JSON request-body template, and the
//! response-version extraction a batched-command API needs to feed its next
//! optimistic-concurrency token back in.
//!
//! All three are declared in the sidecar, so a provider gains a write leg by
//! authoring YAML — the reason this sits in the transport rather than in any
//! one connector's code.

use std::collections::HashMap;
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

fn credential_root() -> holon_mcp_client::CredentialRoot {
    holon_mcp_client::CredentialRoot::new("/tmp/holon-rest-write-config")
}

/// One request the mock received, split into what the assertions need.
#[derive(Clone, Debug)]
struct Seen {
    method: String,
    target: String,
    body: String,
}

struct Mock {
    base_url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Mock {
    fn last(&self) -> Seen {
        self.seen
            .lock()
            .expect("mock request log")
            .last()
            .cloned()
            .expect("the mock received a request")
    }
}

/// Answer every request with a commit-shaped ack. `version` is what the
/// `response_version_path` declaration must find.
const ACK: &str = r#"{"version":74,"pickedItemsVersion":18,"options":{"prices":false}}"#;

async fn start_mock() -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let seen = Arc::new(Mutex::new(Vec::new()));
    let seen_bg = seen.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let seen_conn = seen_bg.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                // Read the head, then exactly `Content-Length` more bytes: a
                // body assertion is worthless if the read can truncate it.
                let head_end = loop {
                    match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                    if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break pos + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                let content_length: usize = head
                    .lines()
                    .find_map(|l| {
                        let (name, value) = l.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().ok())?
                    })
                    .unwrap_or(0);
                while buf.len() < head_end + content_length {
                    match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
                }

                let mut request_line = head.lines().next().unwrap_or_default().split_whitespace();
                let method = request_line.next().unwrap_or_default().to_string();
                let target = request_line.next().unwrap_or_default().to_string();
                let body = String::from_utf8_lossy(&buf[head_end..head_end + content_length]);
                seen_conn.lock().expect("mock request log").push(Seen {
                    method,
                    target,
                    body: body.to_string(),
                });

                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: \
                     {}\r\nConnection: close\r\n\r\n{ACK}",
                    ACK.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    Mock {
        base_url: format!("http://{addr}"),
        seen,
    }
}

// ---------------------------------------------------------------------------
// Sidecars
// ---------------------------------------------------------------------------

/// A batched-command write call: POST, a body template mixing a scalar
/// placeholder with a whole-value one, and the ack's version extracted by a
/// declared path.
fn commit_yaml(base: &str) -> String {
    format!(
        r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: commit
      tool_call_template:
        call_template_type: http
        url: "{base}/api/list/{{listId}}/commit"
        http_method: POST
holon:
  tools:
    commit:
      query:
        version: "{{version}}"
      body:
        oldVersion: "{{version}}"
        lang: en
        commands: "{{commands}}"
      response_version_path: version
entities: {{}}
tools: {{}}
"#
    )
}

fn get_with_body_yaml(base: &str) -> String {
    format!(
        r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: read
      tool_call_template:
        call_template_type: http
        url: "{base}/api/list/{{listId}}"
        http_method: GET
holon:
  tools:
    read:
      body:
        oldVersion: "1"
entities: {{}}
tools: {{}}
"#
    )
}

fn surface_from(yaml: &str) -> anyhow::Result<RestCallSurface> {
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml)?;
    let lookup = |_: &str| None;
    let mcp = cfg.into_mcp_config_with("writer".to_string(), &lookup, &credential_root())?;
    match mcp.transport {
        McpTransport::Rest { manual, .. } => Ok(RestCallSurface::new(manual)),
        other => anyhow::bail!("expected the rest transport, got {other:?}"),
    }
}

async fn call(
    surface: &RestCallSurface,
    name: &'static str,
    args: serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<serde_json::Value> {
    let result = surface
        .call_tool(CallToolRequestParam {
            name: std::borrow::Cow::Borrowed(name),
            arguments: Some(args),
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    holon_mcp_client::mcp_call_surface::extract_tool_response(&result)
}

fn commit_args() -> serde_json::Map<String, serde_json::Value> {
    let mut args = serde_json::Map::new();
    args.insert("listId".into(), serde_json::json!("l-42"));
    args.insert("version".into(), serde_json::json!(73));
    args.insert(
        "commands".into(),
        serde_json::json!([
            {"cmd": "add", "good": {"name": "Oat milk", "cat": "R", "new": true}, "id": "1_0"},
            {"cmd": "del", "good": {"name": "Bread", "cat": "B", "new": true}, "id": "1_1"},
        ]),
    );
    args
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_post_call_sends_the_declared_body_and_method() {
    let mock = start_mock().await;
    let surface = surface_from(&commit_yaml(&mock.base_url)).expect("the commit sidecar resolves");

    call(&surface, "commit", commit_args())
        .await
        .expect("the peer accepts the commit");

    let seen = mock.last();
    assert_eq!(
        seen.method, "POST",
        "the declared method did not reach the wire"
    );
    assert!(
        seen.target.starts_with("/api/list/l-42/commit"),
        "the path placeholder was not filled: {}",
        seen.target
    );
    assert!(
        seen.target.contains("version=73"),
        "the query placeholder was not filled: {}",
        seen.target
    );

    let sent: serde_json::Value =
        serde_json::from_str(&seen.body).expect("the request body is JSON");
    // A placeholder standing ALONE carries the argument's own JSON type: the
    // command array must arrive as an array, and a numeric version as a number.
    assert_eq!(
        sent["commands"].as_array().map(Vec::len),
        Some(2),
        "the whole-value placeholder did not pass the array through: {sent}"
    );
    assert_eq!(sent["commands"][0]["cmd"], serde_json::json!("add"));
    assert_eq!(sent["oldVersion"], serde_json::json!(73));
    assert_eq!(sent["lang"], serde_json::json!("en"));
}

#[tokio::test]
async fn the_declared_response_version_is_extracted() {
    let mock = start_mock().await;
    let surface = surface_from(&commit_yaml(&mock.base_url)).expect("the commit sidecar resolves");

    let response = call(&surface, "commit", commit_args())
        .await
        .expect("the peer accepts the commit");

    // Under a stable key, so a caller needs no knowledge of THIS provider's
    // field name — the whole point of declaring the path in the sidecar.
    assert_eq!(
        response.get(holon_mcp_client::rest_transport::RESPONSE_VERSION_KEY),
        Some(&serde_json::json!(74)),
        "the declared response version was not extracted: {response}"
    );
    // The body still arrives whole; extraction adds, never replaces.
    assert_eq!(response["pickedItemsVersion"], serde_json::json!(18));
}

#[tokio::test]
async fn a_missing_response_version_fails_loud() {
    let mock = start_mock().await;
    let mut yaml = commit_yaml(&mock.base_url);
    yaml = yaml.replace(
        "response_version_path: version",
        "response_version_path: revision",
    );
    let surface = surface_from(&yaml).expect("the sidecar resolves");

    let err = call(&surface, "commit", commit_args())
        .await
        .expect_err("a declared version path that finds nothing cannot be silently skipped");
    assert!(
        format!("{err:#}").contains("revision"),
        "the failure did not name the missing path: {err:#}"
    );
}

#[tokio::test]
async fn a_body_on_a_get_call_is_refused_at_configuration_time() {
    let mock = start_mock().await;
    // Parse, don't validate: a GET carrying a body is a mistake in the YAML,
    // so it must never resolve into a manual that could be called.
    let err = surface_from(&get_with_body_yaml(&mock.base_url))
        .expect_err("a GET with a request body is a configuration error");
    let text = format!("{err:#}");
    assert!(
        text.contains("body") && text.contains("GET"),
        "the refusal did not name the offending shape: {text}"
    );
}

#[tokio::test]
async fn an_unknown_method_is_refused_at_configuration_time() {
    let mock = start_mock().await;
    let yaml = commit_yaml(&mock.base_url).replace("http_method: POST", "http_method: TRACE");
    let err = surface_from(&yaml).expect_err("TRACE is not a method this transport issues");
    assert!(
        format!("{err:#}").contains("TRACE"),
        "the refusal did not name the method: {err:#}"
    );
}

#[tokio::test]
async fn a_placeholder_with_no_argument_fails_loud_in_the_body() {
    let mock = start_mock().await;
    let surface = surface_from(&commit_yaml(&mock.base_url)).expect("the commit sidecar resolves");

    let mut args = commit_args();
    args.remove("commands");
    let err = call(&surface, "commit", args)
        .await
        .expect_err("an unfilled body placeholder must not be sent as literal text");
    assert!(
        format!("{err:#}").contains("commands"),
        "the failure did not name the unfilled placeholder: {err:#}"
    );
}

#[tokio::test]
async fn a_delete_call_carries_no_body_and_still_reaches_the_wire() {
    let mock = start_mock().await;
    let yaml = format!(
        r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: commit
      tool_call_template:
        call_template_type: http
        url: "{}/api/list/{{listId}}/commit"
        http_method: DELETE
entities: {{}}
tools: {{}}
"#,
        mock.base_url
    );
    let surface = surface_from(&yaml).expect("the delete sidecar resolves");

    call(&surface, "commit", commit_args())
        .await
        .expect("the peer accepts the delete");

    let seen = mock.last();
    assert_eq!(seen.method, "DELETE");
    assert!(
        seen.body.is_empty(),
        "a bodyless call sent one: {}",
        seen.body
    );
}

#[test]
fn the_known_methods_round_trip_through_the_config() {
    let mut round_tripped = HashMap::new();
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        let yaml = format!(
            r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: c
      tool_call_template:
        call_template_type: http
        url: "http://127.0.0.1:1/x"
        http_method: {method}
entities: {{}}
tools: {{}}
"#
        );
        let cfg: IntegrationFileConfig =
            serde_yaml::from_str(&yaml).expect("the method sidecar parses");
        let lookup = |_: &str| None;
        let mcp = cfg
            .into_mcp_config_with("m".to_string(), &lookup, &credential_root())
            .unwrap_or_else(|e| panic!("{method} must resolve: {e:#}"));
        match mcp.transport {
            McpTransport::Rest { manual, .. } => {
                round_tripped.insert(method, manual.calls["c"].method.to_string());
            }
            other => panic!("expected rest, got {other:?}"),
        }
    }
    for method in ["GET", "POST", "PUT", "PATCH", "DELETE"] {
        assert_eq!(round_tripped.get(method).map(String::as_str), Some(method));
    }
}

//! A `rest` connection's DECLARED write tool, dispatched through the operation
//! provider: the sidecar's method and body template reach the wire, the
//! declared `response_version_path` comes back on the result, and the
//! `writes:` policy still refuses a non-read effect the sidecar did not enable.
//!
//! Both legs run against a local mock server (no network). The dispatch goes
//! through the provider — the chokepoint that carries the write policy, the
//! intent key and the pending-write store — rather than through the sync path,
//! which is what makes the guard rails below reachable at all.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;

use holon_api::EntityName;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::OperationProvider;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpOperationProvider;
use holon_mcp_client::McpSidecar;
use holon_mcp_client::McpTransport;
use holon_mcp_client::rest_transport::RESPONSE_VERSION_KEY;
use holon_mcp_client::rest_transport::RestCallSurface;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

fn credential_root() -> holon_mcp_client::CredentialRoot {
    holon_mcp_client::CredentialRoot::new("/tmp/holon-rest-write-dispatch-config")
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

    fn request_count(&self) -> usize {
        self.seen.lock().expect("mock request log").len()
    }
}

/// Answer every request with an insert-shaped ack. `version` is what the
/// `response_version_path` declaration must find.
const ACK: &str = r#"{"id":"item-1","version":74}"#;

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

/// The `inputs:` document the manual publishes for `add-item`.
const ADD_ITEM_INPUTS: &str = r#"      inputs:
        type: object
        required: [listId, name]
        properties:
          listId:
            type: string
            description: The list to add to.
          name:
            type: string
            description: The item's name.
"#;

/// One POST tool the manual publishes, declared in the `holon:` section with a
/// body template and a version path. `writes` is the transport-level policy the
/// sidecar states; `publishes_inputs` drops the manual's `inputs:` document.
fn sidecar_yaml(base: &str, writes: &str, publishes_inputs: bool) -> String {
    let inputs = if publishes_inputs {
        ADD_ITEM_INPUTS
    } else {
        ""
    };
    format!(
        r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: add-item
      description: Add one item to a list.
{inputs}      tool_call_template:
        call_template_type: http
        url: "{base}/api/list/{{listId}}/items"
        http_method: POST
holon:
  tools:
    add-item:
      query:
        version: "{{version}}"
      body:
        oldVersion: "{{version}}"
        name: "{{name}}"
        lang: en
      response_version_path: version
entities:
  rest_item:
    short_name: item
    id_column: id
    schema:
      - {{ name: id, sql_type: TEXT, primary_key: true }}
      - {{ name: name, sql_type: TEXT }}
writes: {writes}
tools:
  add-item:
    entity: rest_item
    display_name: Add item
    effect: idempotent
    affected_fields: [name]
"#
    )
}

/// The surface and sidecar a `rest` file resolves to — the same two halves
/// `finish_rest_integration` builds.
fn config_from(yaml: &str) -> (RestCallSurface, McpSidecar) {
    let cfg: IntegrationFileConfig = serde_yaml::from_str(yaml).expect("the sidecar parses");
    let lookup = |_: &str| None;
    let mcp = cfg
        .into_mcp_config_with("rest_writer".to_string(), &lookup, &credential_root())
        .expect("the rest sidecar resolves");
    let sidecar = McpSidecar::from_yaml(&mcp.sidecar_yaml).expect("the sidecar validates");
    match mcp.transport {
        McpTransport::Rest { manual, .. } => (RestCallSurface::new(manual), sidecar),
        other => panic!("expected the rest transport, got {other:?}"),
    }
}

/// The provider a `rest` connection dispatches through, built the way
/// `finish_rest_integration` builds it.
fn dispatch_provider(surface: Arc<RestCallSurface>, sidecar: McpSidecar) -> McpOperationProvider {
    McpOperationProvider::rest(surface, sidecar, HashMap::new()).expect("the rest provider builds")
}

fn add_item_params() -> StorageEntity {
    let mut params = StorageEntity::new();
    params.insert("id".into(), Value::String("item-1".to_string()));
    params.insert("listId".into(), Value::String("l-42".to_string()));
    params.insert("version".into(), Value::Integer(73));
    params.insert("name".into(), Value::String("Oat milk".to_string()));
    params
}

#[tokio::test]
async fn a_declared_rest_write_reaches_the_wire_with_its_method_and_body() {
    let mock = start_mock().await;
    let (surface, sidecar) = config_from(&sidecar_yaml(&mock.base_url, "enabled", true));
    let provider = dispatch_provider(Arc::new(surface), sidecar);

    let result = provider
        .execute_operation(&EntityName::new("rest_item"), "add_item", add_item_params())
        .await
        .expect("the declared write dispatches");

    let seen = mock.last();
    assert_eq!(seen.method, "POST", "the declared method reached the wire");
    assert!(
        seen.target.starts_with("/api/list/l-42/items"),
        "the url placeholder was filled: {}",
        seen.target
    );
    let body: serde_json::Value =
        serde_json::from_str(&seen.body).expect("the body template rendered as JSON");
    assert_eq!(body["name"], serde_json::json!("Oat milk"));
    assert_eq!(body["lang"], serde_json::json!("en"));
    assert_eq!(body["oldVersion"], serde_json::json!(73));

    let response: serde_json::Value = result
        .response
        .expect("the ack is carried back on the result")
        .into();
    assert_eq!(
        response[RESPONSE_VERSION_KEY],
        serde_json::json!(74),
        "the declared response_version_path is read back under {RESPONSE_VERSION_KEY}"
    );
}

#[tokio::test]
async fn a_sidecar_with_writes_disabled_still_refuses_the_effect() {
    let mock = start_mock().await;
    let (surface, sidecar) = config_from(&sidecar_yaml(&mock.base_url, "disabled", true));
    let provider = dispatch_provider(Arc::new(surface), sidecar);

    let err = provider
        .execute_operation(&EntityName::new("rest_item"), "add_item", add_item_params())
        .await
        .expect_err("a non-read effect under `writes: disabled` is refused");

    let message = err.to_string();
    assert!(
        message.contains("needs `writes: enabled`"),
        "the writes gate refused it, not the transport: {message}"
    );
    assert_eq!(
        mock.request_count(),
        0,
        "a refused write never reaches the wire"
    );
}

#[test]
fn a_mutating_tool_the_manual_does_not_publish_is_refused() {
    let (surface, _) = config_from(&sidecar_yaml("https://example.invalid", "enabled", true));
    let sidecar = McpSidecar::from_yaml(
        "entities:\n  rest_item:\n    id_column: id\nwrites: enabled\ntools:\n  add-item-x:\n    \
         entity: rest_item\n    effect: idempotent\n",
    )
    .expect("the sidecar validates");

    let err = match McpOperationProvider::rest(Arc::new(surface), sidecar, HashMap::new()) {
        Ok(_) => panic!("a declared write naming no published call must be refused"),
        Err(e) => e,
    };

    let message = err.to_string();
    assert!(
        message.contains("add-item-x"),
        "the refusal names the tool that names no call: {message}"
    );
}

#[test]
fn a_mutating_tool_that_publishes_no_inputs_is_refused() {
    let (surface, sidecar) =
        config_from(&sidecar_yaml("https://example.invalid", "enabled", false));

    let err = match McpOperationProvider::rest(Arc::new(surface), sidecar, HashMap::new()) {
        Ok(_) => panic!("a declared write with no published inputs must be refused"),
        Err(e) => e,
    };

    let message = err.to_string();
    assert!(
        message.contains("publishes no `inputs:`"),
        "the refusal names the missing declaration: {message}"
    );
}

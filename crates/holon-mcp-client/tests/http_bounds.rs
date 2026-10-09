//! A peer decides how much it sends and how long it takes; the client decides
//! how much it reads and how long it waits.

use std::net::SocketAddr;

use holon_mcp_client::CredentialRoot;
use holon_mcp_client::MAX_RESPONSE_BODY_BYTES;
use holon_mcp_client::McpTransport;
use holon_mcp_client::REQUEST_TIMEOUT;
use holon_mcp_client::RestCallSurface;
use holon_mcp_client::integration_config::IntegrationFileConfig;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use rmcp::model::CallToolRequestParam;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

fn surface_calling(url: &str) -> RestCallSurface {
    let yaml = format!(
        r#"
schema_version: 2
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: list
      tool_call_template:
        call_template_type: http
        http_method: GET
        url: "{url}"
holon:
  tools:
    list: {{}}
"#
    );
    let cfg: IntegrationFileConfig = serde_yaml::from_str(&yaml).expect("fixture parses");
    let built = cfg
        .into_mcp_config_with(
            "fixture".to_string(),
            &|_| None,
            &CredentialRoot::new("/tmp/holon-http-bounds-test"),
        )
        .expect("a loopback URL builds");
    match built.transport {
        McpTransport::Rest { manual, .. } => RestCallSurface::new(manual),
        other => panic!("expected a rest transport, got {other:?}"),
    }
}

fn list_call() -> CallToolRequestParam {
    CallToolRequestParam {
        name: "list".into(),
        arguments: Some(serde_json::Map::new()),
    }
}

/// Answers once with a JSON document of `string_len` bytes of string content,
/// framed by connection close so no `Content-Length` announces the size.
async fn oversized_json_server(string_len: usize) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n";
        if sock.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        let _ = sock.write_all(br#"{"x":""#).await;
        let chunk = vec![b'a'; 1024 * 1024];
        let mut left = string_len;
        while left > 0 {
            let n = left.min(chunk.len());
            if sock.write_all(&chunk[..n]).await.is_err() {
                return;
            }
            left -= n;
        }
        let _ = sock.write_all(br#""}"#).await;
        let _ = sock.shutdown().await;
    });
    addr
}

/// Accepts, reads the request, and never answers.
async fn silent_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        std::future::pending::<()>().await;
        drop(sock);
    });
    addr
}

#[tokio::test]
async fn a_body_past_the_cap_is_refused_while_it_streams() {
    let addr = oversized_json_server(MAX_RESPONSE_BODY_BYTES + 1024 * 1024).await;
    let surface = surface_calling(&format!("http://{addr}/list"));
    let msg = match surface.call_tool(list_call()).await {
        Ok(_) => panic!(
            "a body larger than MAX_RESPONSE_BODY_BYTES ({MAX_RESPONSE_BODY_BYTES}) was read whole"
        ),
        Err(e) => format!("{e:#}"),
    };
    assert!(
        msg.contains("MAX_RESPONSE_BODY_BYTES"),
        "the refusal must name the body cap; got: {msg}"
    );
}

/// Paused time: the clock jumps to the next timer whenever the runtime idles,
/// so the client's own deadline fires at once if it has one, and the outer
/// bound fires if it does not.
#[tokio::test(start_paused = true)]
async fn a_peer_that_never_answers_ends_in_a_timeout_error() {
    let addr = silent_server().await;
    let surface = surface_calling(&format!("http://{addr}/list"));
    let started = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(REQUEST_TIMEOUT * 2, surface.call_tool(list_call()))
        .await
        .unwrap_or_else(|_| {
            panic!("the client was still waiting after twice REQUEST_TIMEOUT ({REQUEST_TIMEOUT:?})")
        });
    let waited = started.elapsed();
    let msg = match outcome {
        Ok(r) => panic!("a peer that never answered produced a result: {r:?}"),
        Err(e) => format!("{e:#}"),
    };
    assert!(
        msg.contains("REQUEST_TIMEOUT"),
        "the error must name the request timeout; got: {msg}"
    );
    assert!(
        waited <= REQUEST_TIMEOUT + std::time::Duration::from_secs(1),
        "the client gave up after {waited:?}, past REQUEST_TIMEOUT ({REQUEST_TIMEOUT:?})"
    );
}

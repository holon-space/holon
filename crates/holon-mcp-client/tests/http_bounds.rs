//! A peer decides how much it sends and how long it takes; the client decides
//! how much it reads and how long it waits.

use std::net::SocketAddr;

use holon_mcp_client::CredentialRoot;
use holon_mcp_client::MAX_RESPONSE_BODY_BYTES;
use holon_mcp_client::MCP_IDLE_TIMEOUT;
use holon_mcp_client::McpTransport;
use holon_mcp_client::REQUEST_TIMEOUT;
use holon_mcp_client::RestCallSurface;
use holon_mcp_client::connect_mcp;
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

const JSON_HEAD: &str =
    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n";

/// Answers once with a JSON document of `string_len` bytes of string content,
/// framed by connection close so no `Content-Length` announces the size.
async fn oversized_json_server(string_len: usize) -> SocketAddr {
    oversized_server(JSON_HEAD, br#"{"x":""#, string_len, br#""}"#).await
}

/// Answers once with `head`, then `prefix`, `filler_len` bytes of `a` and
/// `suffix`, then closes.
async fn oversized_server(
    head: &'static str,
    prefix: &'static [u8],
    filler_len: usize,
    suffix: &'static [u8],
) -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        if sock.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        let _ = sock.write_all(prefix).await;
        let chunk = vec![b'a'; 1024 * 1024];
        let mut left = filler_len;
        while left > 0 {
            let n = left.min(chunk.len());
            if sock.write_all(&chunk[..n]).await.is_err() {
                return;
            }
            left -= n;
        }
        let _ = sock.write_all(suffix).await;
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

async fn mcp_connect_error(addr: SocketAddr) -> String {
    match connect_mcp(&format!("http://{addr}/mcp"), None).await {
        Ok(_) => panic!("an MCP peer that sent no initialize result was connected"),
        Err(e) => format!("{e:#}"),
    }
}

#[tokio::test]
async fn an_mcp_json_message_past_the_cap_is_refused_while_it_streams() {
    let addr = oversized_json_server(MAX_RESPONSE_BODY_BYTES + 1024 * 1024).await;
    let msg = mcp_connect_error(addr).await;
    assert!(
        msg.contains("MAX_RESPONSE_BODY_BYTES"),
        "the refusal must name the body cap; got: {msg}"
    );
}

/// Sends one SSE event of more than [`MAX_RESPONSE_BODY_BYTES`], as 1 MiB
/// `data:` lines with no blank line to end it, and keeps the connection open.
async fn endless_event_server() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        let head = "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
        if sock.write_all(head.as_bytes()).await.is_err() {
            return;
        }
        let mut line = b"data: ".to_vec();
        line.extend(std::iter::repeat_n(b'a', 1024 * 1024));
        line.push(b'\n');
        for _ in 0..=MAX_RESPONSE_BODY_BYTES / (1024 * 1024) {
            if sock.write_all(&line).await.is_err() {
                return;
            }
        }
        std::future::pending::<()>().await;
        drop(sock);
    });
    addr
}

/// The peer never ends the event and never closes, so only the cap can end
/// the connect. rmcp reports a failed initialize stream as "connection
/// closed" without its cause.
#[tokio::test]
async fn an_mcp_event_past_the_cap_ends_the_stream() {
    let addr = endless_event_server().await;
    let bound = std::time::Duration::from_secs(60);
    tokio::time::timeout(bound, mcp_connect_error(addr))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the MCP client was still reading one event after {bound:?}, past \
                 MAX_RESPONSE_BODY_BYTES ({MAX_RESPONSE_BODY_BYTES})"
            )
        });
}

/// Paused time, as in `a_peer_that_never_answers_ends_in_a_timeout_error`.
#[tokio::test(start_paused = true)]
async fn an_mcp_peer_that_never_answers_ends_in_an_idle_timeout_error() {
    let addr = silent_server().await;
    let started = tokio::time::Instant::now();
    let msg = tokio::time::timeout(MCP_IDLE_TIMEOUT * 2, mcp_connect_error(addr))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "the MCP client was still waiting after twice MCP_IDLE_TIMEOUT \
                 ({MCP_IDLE_TIMEOUT:?})"
            )
        });
    let waited = started.elapsed();
    assert!(
        msg.contains("MCP_IDLE_TIMEOUT"),
        "the error must name the idle timeout; got: {msg}"
    );
    assert!(
        waited <= MCP_IDLE_TIMEOUT + std::time::Duration::from_secs(1),
        "the client gave up after {waited:?}, past MCP_IDLE_TIMEOUT ({MCP_IDLE_TIMEOUT:?})"
    );
}

#[tokio::test]
async fn an_mcp_redirect_off_https_is_refused() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut buf = [0u8; 4096];
        let _ = sock.read(&mut buf).await;
        let _ = sock
            .write_all(
                b"HTTP/1.1 307 Temporary Redirect\r\nLocation: http://redirect-target.invalid/mcp\r\n\
                  Content-Length: 0\r\n\r\n",
            )
            .await;
    });
    let msg = mcp_connect_error(addr).await;
    assert!(
        msg.contains("refused a redirect"),
        "the error must say the redirect hop was refused; got: {msg}"
    );
}

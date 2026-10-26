//! A web page that rebinds its DNS name to 127.0.0.1 reaches the loopback MCP
//! server with its own name in `Host` and `Origin`. The server must refuse such
//! a request before the MCP service issues a session.

use std::sync::Arc;
use std::time::Duration;

use holon_mcp::di::serve_http_on;
use holon_mcp::server::DebugServices;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio_util::sync::CancellationToken;

const INITIALIZE: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"host-origin-test","version":"0"}}}"#;

struct Server {
    port: u16,
    stop: CancellationToken,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

async fn serve() -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind an ephemeral loopback port");
    let port = listener.local_addr().expect("local addr").port();
    let stop = CancellationToken::new();
    let served = stop.clone();
    tokio::spawn(async move {
        serve_http_on(
            None,
            Arc::new(DebugServices::default()),
            None,
            None,
            listener,
            Arc::new(LocalSessionManager::default()),
            served,
        )
        .await
        .expect("serve the MCP router");
    });
    Server { port, stop }
}

struct Answer {
    status: u16,
    session_id: Option<String>,
    head: String,
}

/// Sends `initialize` with exactly these `Host` and `Origin` headers and reads
/// the response head only: an accepted `initialize` answers with an SSE stream.
async fn initialize(port: u16, host: &str, origin: Option<&str>) -> Answer {
    let origin_line = origin.map_or(String::new(), |o| format!("Origin: {o}\r\n"));
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: {host}\r\n{origin_line}Content-Type: application/json\r\n\
         Accept: application/json, text/event-stream\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n{INITIALIZE}",
        INITIALIZE.len()
    );
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect to the MCP server");
    stream
        .write_all(request.as_bytes())
        .await
        .expect("send initialize");
    let mut received = Vec::new();
    let mut chunk = [0u8; 4096];
    tokio::time::timeout(Duration::from_secs(30), async {
        while !received.windows(4).any(|w| w == b"\r\n\r\n") {
            let n = stream.read(&mut chunk).await.expect("read the response");
            assert!(n > 0, "connection closed before the response head ended");
            received.extend_from_slice(&chunk[..n]);
        }
    })
    .await
    .expect("response head within 30 s");
    let text = String::from_utf8_lossy(&received);
    let head = text.split("\r\n\r\n").next().unwrap().to_string();
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("no status in response head:\n{head}"));
    let session_id = head.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        name.eq_ignore_ascii_case("mcp-session-id")
            .then(|| value.trim().to_string())
    });
    Answer {
        status,
        session_id,
        head,
    }
}

fn assert_refused(answer: &Answer) {
    assert_eq!(answer.status, 403, "expected 403, got:\n{}", answer.head);
    assert_eq!(
        answer.session_id, None,
        "a refused request got a session:\n{}",
        answer.head
    );
}

fn assert_accepted(answer: &Answer) {
    assert_eq!(answer.status, 200, "expected 200, got:\n{}", answer.head);
    assert!(
        answer.session_id.is_some(),
        "an accepted initialize got no session:\n{}",
        answer.head
    );
}

#[tokio::test]
async fn a_foreign_host_is_refused() {
    let server = serve().await;
    let port = server.port;
    assert_refused(&initialize(port, &format!("evil.example:{port}"), None).await);
}

#[tokio::test]
async fn a_foreign_origin_is_refused() {
    let server = serve().await;
    let port = server.port;
    assert_refused(
        &initialize(
            port,
            &format!("127.0.0.1:{port}"),
            Some(&format!("http://evil.example:{port}")),
        )
        .await,
    );
}

#[tokio::test]
async fn a_loopback_host_without_origin_is_served() {
    let server = serve().await;
    let port = server.port;
    for host in ["127.0.0.1", "localhost", "[::1]"] {
        assert_accepted(&initialize(port, &format!("{host}:{port}"), None).await);
    }
}

#[tokio::test]
async fn a_loopback_host_with_a_loopback_origin_is_served() {
    let server = serve().await;
    let port = server.port;
    for host in ["127.0.0.1", "localhost", "[::1]"] {
        let authority = format!("{host}:{port}");
        assert_accepted(&initialize(port, &authority, Some(&format!("http://{authority}"))).await);
    }
}

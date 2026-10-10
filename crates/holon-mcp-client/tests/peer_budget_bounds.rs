//! Enumerating a peer's tools is a loop the PEER controls: it decides how many
//! pages there are, how many items each holds, and how long each takes. Every
//! connect runs it (`McpOperationProvider::connect` → `from_peer_shared`), so a
//! peer that always answers with one more `nextCursor` grows the process until
//! it dies — with the per-request deadline satisfied the whole time.
//!
//! The bounds are the connection's peer budget: a page count, an item count and
//! a total deadline covering the handshake and every page of the connect.

use std::collections::HashMap;
use std::io::Write as _;

use holon_mcp_client::McpSidecar;
use holon_mcp_client::mcp_provider::McpOperationProvider;

/// A hostile peer answers a page in microseconds, so a bound that holds ends
/// the connect long inside this. Red, it never ends at all.
const BOUND: std::time::Duration = std::time::Duration::from_secs(20);

/// A peer that answers `initialize` and then answers EVERY `tools/list` with
/// `tools_per_page` tools and always one more cursor.
///
/// Speaks Streamable HTTP over a raw socket rather than through `rmcp`'s server
/// so that it can answer what no cooperative implementation would.
fn endless_pagination_peer(tools_per_page: usize) -> String {
    peer(tools_per_page, true)
}

/// A peer that answers `initialize` and hands out its whole tool list in one
/// page, so a connect against it succeeds.
fn finite_peer() -> String {
    peer(1, false)
}

fn peer(tools_per_page: usize, endless: bool) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the hostile peer");
    let uri = format!("http://{}/mcp", listener.local_addr().expect("peer addr"));
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            std::thread::spawn(move || serve(stream, tools_per_page, endless));
        }
    });
    uri
}

fn serve(mut stream: std::net::TcpStream, tools_per_page: usize, endless: bool) {
    use std::io::BufRead as _;
    let peer = stream.try_clone().expect("clone the socket");
    let mut head = std::io::BufReader::new(peer);
    loop {
        let mut length = 0;
        let mut is_post = false;
        loop {
            let mut line = String::new();
            if head.read_line(&mut line).unwrap_or(0) == 0 {
                return;
            }
            if line == "\r\n" {
                break;
            }
            is_post |= line.starts_with("POST ");
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse::<usize>().expect("a numeric Content-Length");
            }
        }
        if !is_post {
            // The notification GET stream. Refusing it keeps this peer to the
            // one leg under test.
            stream
                .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")
                .expect("refuse the GET stream");
            continue;
        }
        let mut body = vec![0u8; length];
        std::io::Read::read_exact(&mut head, &mut body).expect("the announced body arrives");
        let msg: serde_json::Value = serde_json::from_slice(&body).expect("a JSON-RPC body");
        let Some(method) = msg["method"].as_str() else {
            return;
        };
        if msg["id"].is_null() {
            // A notification: nothing to answer.
            stream
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                .expect("ack a notification");
            continue;
        }
        let result = match method {
            "initialize" => serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "endless", "version": "0"},
            }),
            "tools/list" => {
                let mut page = serde_json::json!({
                    "tools": (0..tools_per_page)
                        .map(|i| serde_json::json!({
                            "name": format!("tool_{i}"),
                            "description": "a tool",
                            "inputSchema": {"type": "object"},
                        }))
                        .collect::<Vec<_>>(),
                });
                if endless {
                    page["nextCursor"] = serde_json::json!("there is always one more page");
                }
                page
            }
            _ => serde_json::json!({}),
        };
        let reply = serde_json::json!({"jsonrpc": "2.0", "id": msg["id"], "result": result});
        let body = serde_json::to_vec(&reply).expect("serialize the reply");
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: \
                     endless\r\nContent-Length: {}\r\n\r\n",
                    body.len()
                )
                .as_bytes(),
            )
            .expect("write the reply head");
        stream.write_all(&body).expect("write the reply body");
    }
}

fn empty_sidecar() -> McpSidecar {
    serde_yaml::from_str::<McpSidecar>("{}").expect("an empty sidecar parses")
}

/// What `connect` reports against a peer that paginates forever, or a panic
/// naming what was still running.
async fn connect_failure(uri: &str) -> String {
    let outcome = tokio::time::timeout(
        BOUND,
        McpOperationProvider::connect(uri, None, empty_sidecar(), HashMap::new()),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "the connect was still enumerating tools after {BOUND:?} — a peer that always answers \
             with one more `nextCursor` decides how much of this process it owns, and every \
             connect runs this loop"
        )
    });
    format!(
        "{:#}",
        outcome
            .err()
            .expect("a peer that never finishes its tool list is no usable connection")
    )
}

/// One tool per page: the ITEM count stays small, so only a page count stops
/// it. An empty page with a cursor is the same shape and the same attack.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_paginates_in_small_pages_is_cut_at_the_page_bound() {
    let err = connect_failure(&endless_pagination_peer(1)).await;
    assert!(
        err.contains("MAX_LIST_PAGES"),
        "the refusal must name the bound it hit, or nobody can tell a hostile peer from a slow \
         one; got: {err}"
    );
}

/// Full pages: the item count is reached first, and it is the one that bounds
/// the bytes — the page bound alone lets a peer send 50 MiB per page.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_that_paginates_in_full_pages_is_cut_at_the_item_bound() {
    let err = connect_failure(&endless_pagination_peer(1024)).await;
    assert!(
        err.contains("MAX_LIST_ITEMS"),
        "the refusal must name the bound it hit; got: {err}"
    );
}

/// `finish_integration` runs TWO enumerations against ONE budget
/// (`list_all_resource_templates`, then `from_peer_shared` ->
/// `list_all_tools`), and the PEER owns the time the first one takes: it
/// decides how many pages and how many templates — each of which becomes a
/// cache table and a DDL — there are. A spent budget is therefore a state a
/// peer can drive the connect into, so it has to read like every other bound
/// here: refused, naming CONNECT_BUDGET.
#[tokio::test]
async fn an_enumeration_starting_past_the_budget_is_refused_naming_the_bound() {
    let (peer, _service) = holon_mcp_client::connect_mcp(&finite_peer(), None)
        .await
        .expect("a peer that lists its tools in one page connects");
    // The peer spent the connect budget on its first enumeration. Paused only
    // now, so the connect above ran on the real clock.
    tokio::time::pause();
    tokio::time::advance(holon_mcp_client::CONNECT_BUDGET + std::time::Duration::from_secs(1))
        .await;
    let outcome =
        McpOperationProvider::from_peer_shared(peer, empty_sidecar(), HashMap::new()).await;
    let err = format!(
        "{:#}",
        outcome
            .err()
            .expect("no provider from an enumeration that cannot run")
    );
    assert!(
        err.contains("CONNECT_BUDGET"),
        "the refusal must name the bound it hit; got: {err}"
    );
}

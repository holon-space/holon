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
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

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
    peer(tools_per_page, true, SMALL_TOOL).0
}

/// A peer whose pages stay well inside the page and item bounds and carry a
/// megabyte of description per tool.
fn fat_pagination_peer() -> String {
    peer(16, true, 1 << 20).0
}

/// A peer that answers `initialize` and hands out its whole tool list in one
/// page, so a connect against it succeeds.
fn finite_peer() -> String {
    peer(1, false, SMALL_TOOL).0
}

/// A peer that hands out [`HUGE_PAGE_BYTES`] of tools in its FIRST page, so
/// nothing but the read itself can stop it.
fn one_huge_page_peer() -> (String, Arc<AtomicUsize>) {
    peer(HUGE_PAGE_TOOLS, false, 1 << 20)
}

/// Tools of a megabyte each, well past `MAX_LIST_BYTES` and well inside
/// `MAX_RESPONSE_BODY_BYTES`.
const HUGE_PAGE_TOOLS: usize = 40;
const HUGE_PAGE_BYTES: usize = HUGE_PAGE_TOOLS * (1 << 20);

/// The description length of a tool no bound is meant to catch.
const SMALL_TOOL: usize = 6;

/// The peer's URI and the reply bytes it has managed to hand over, which is
/// what tells a body cut off mid-read from one read whole and refused after.
fn peer(
    tools_per_page: usize,
    endless: bool,
    description_bytes: usize,
) -> (String, Arc<AtomicUsize>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the hostile peer");
    let uri = format!("http://{}/mcp", listener.local_addr().expect("peer addr"));
    let handed_over = Arc::new(AtomicUsize::new(0));
    let counting = handed_over.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { continue };
            let counting = counting.clone();
            std::thread::spawn(move || {
                serve(
                    stream,
                    tools_per_page,
                    endless,
                    description_bytes,
                    &counting,
                )
            });
        }
    });
    (uri, handed_over)
}

fn serve(
    mut stream: std::net::TcpStream,
    tools_per_page: usize,
    endless: bool,
    description_bytes: usize,
    handed_over: &AtomicUsize,
) {
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
                            "description": "d".repeat(description_bytes),
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
        for chunk in body.chunks(64 * 1024) {
            if stream.write_all(chunk).is_err() {
                // The client stopped reading: nothing more of this reply gets
                // out, and how much did is the measurement.
                return;
            }
            handed_over.fetch_add(chunk.len(), Ordering::Relaxed);
        }
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

/// Small pages of huge items: 16 tools of a megabyte each stays under the item
/// count AND under the page count, so the counts alone let a peer hand over
/// gigabytes — 4096 items and 256 pages of this shape are ~16 GiB.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_whose_pages_are_huge_is_cut_at_the_list_byte_bound() {
    let err = connect_failure(&fat_pagination_peer()).await;
    assert!(
        err.contains("MAX_LIST_BYTES"),
        "the refusal must name the bound it hit; got: {err}"
    );
}

/// `MAX_RESPONSE_BODY_BYTES` is four times `MAX_LIST_BYTES`, so a single page
/// the enumeration can never hold still fits one reply body: charging the
/// items only once they are decoded means receiving and deserializing all of
/// it first, and the transient peak — the body plus the decoded items — is the
/// peer's choice, not the bound's.
#[tokio::test(flavor = "multi_thread")]
async fn a_single_page_past_the_list_allowance_is_cut_off_during_the_read() {
    let (uri, handed_over) = one_huge_page_peer();
    let err = connect_failure(&uri).await;
    assert!(
        err.contains("MAX_LIST_BYTES"),
        "the refusal must name the bound it hit; got: {err}"
    );
    let got_out = handed_over.load(Ordering::Relaxed);
    assert!(
        got_out < HUGE_PAGE_BYTES,
        "a {HUGE_PAGE_BYTES}-byte page cannot fit MAX_LIST_BYTES, so the read must stop inside \
         it; the peer handed over {got_out} bytes"
    );
}

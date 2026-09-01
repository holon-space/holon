//! The shopping read leg end-to-end, over the REAL `rest` transport against a
//! LOCAL mock HTTP server (no network).
//!
//! Three things are under test and none can be checked in isolation: that a
//! served list becomes `shopping_item` rows, that an incomplete fetch changes
//! NOTHING (absence is only a deletion signal inside a complete snapshot), and
//! that the credential in the URL path never reaches an error string.
//!
//! The rotating-token shape of the phone API is a TRANSPORT guarantee, not a
//! kitchen one; it is pinned in
//! `crates/holon-mcp-client/tests/rest_transport_redaction.rs`.
//!
//! The mock is a hand-rolled HTTP/1.1 server on a `TcpListener`, mirroring
//! `crates/holon-mcp-client/tests/rest_transport_mock.rs`, so the test pulls in
//! no HTTP-server dependency.

use std::borrow::Cow;

use holon_kitchen::shopping::CategoryCode;
use holon_kitchen::shopping::CompleteSnapshot;
use holon_kitchen::shopping::KnownCategory;
use holon_kitchen::shopping::LocalIntent;
use holon_kitchen::shopping::ShoppingReconciler;
use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::rest_transport::RestCallSurface;
use rmcp::model::CallToolRequestParam;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// The credential: an opaque token this endpoint carries in a URL PATH segment.
/// Synthetic — a captured token never belongs in a fixture.
const CAP_TOKEN: &str = "cap-7f3a9d2e4b8c1056";
const LIST_ID: &str = "l-42";
const FETCHED_AT: &str = "2026-09-01T10:00:00Z";

/// The bundled sidecar under test.
const SHOPPING_SIDECAR: &str = include_str!("../../../assets/integrations/shopping.yaml");

// ---------------------------------------------------------------------------
// Mock peer
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
enum Mode {
    /// A well-formed full snapshot, including a duplicate key and a category
    /// code this build does not know.
    FullList,
    /// 200, but the body stops mid-JSON — a fetch that "succeeded" and is not
    /// complete.
    TruncatedBody,
    /// The upstream failed and echoed the request URL back at us.
    EchoUrlIn500,
}

fn full_list_body() -> String {
    // Two "Milk" rows in one category: the peer allows duplicates the
    // `(name, cat)` key cannot distinguish, so ingest folds their counts.
    // `Fish` appears in the peer's shipped aisle order but not in its label
    // table, so it is a real unrecognized code, not a hypothetical one.
    r#"{"data":{"items":[
        {"name":"Milk","cat":"R","count":2},
        {"name":"Milk","cat":"R"},
        {"name":"Milk","cat":"Ca","count":1},
        {"name":"Bread","cat":"B"},
        {"name":"Salmon","cat":"Fish","count":1},
        {"name":"Screws","cat":"Ir_shed","count":30}
    ]}}"#
        .to_string()
}

async fn start_mock(mode: Mode) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => buf.extend_from_slice(&tmp[..n]),
                    }
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

                let (status, body) = match mode {
                    Mode::FullList => ("200 OK", full_list_body()),
                    Mode::TruncatedBody => {
                        let full = full_list_body();
                        ("200 OK", full[..full.len() / 2].to_string())
                    }
                    Mode::EchoUrlIn500 => (
                        "500 Internal Server Error",
                        format!("{{\"error\":\"upstream failed for {path}\"}}"),
                    ),
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: \
                     {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes()).await;
                let _ = socket.flush().await;
            });
        }
    });

    format!("http://{addr}/c/{CAP_TOKEN}")
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Drive the SHIPPED sidecar, with its one `${VAR}` pointed at the mock. The
/// whole base URL is the credential here, so the whole base URL is the
/// registered secret.
fn surface_for(base_url: &str) -> RestCallSurface {
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(SHOPPING_SIDECAR).expect("the shipped shopping sidecar parses");
    let lookup = |name: &str| match name {
        "SHOPPING_LIST_URL" => Some(base_url.to_string()),
        _ => None,
    };
    let mcp = cfg
        .into_mcp_config_with(
            "shopping".to_string(),
            &lookup,
            // The sidecar declares no credential FILE, so nothing is read from
            // this root; confinement itself is covered in the mcp-client tests.
            &CredentialRoot::new("/tmp/holon-shopping-c1-config"),
        )
        .expect("shopping sidecar resolves into an mcp config");
    match mcp.transport {
        McpTransport::Rest { manual, .. } => RestCallSurface::new(manual),
        other => panic!("expected the rest transport, got {other:?}"),
    }
}

async fn list_items(surface: &RestCallSurface) -> anyhow::Result<serde_json::Value> {
    let mut args = serde_json::Map::new();
    args.insert("listId".into(), serde_json::Value::String(LIST_ID.into()));
    let result = surface
        .call_tool(CallToolRequestParam {
            name: Cow::Borrowed("list-items"),
            arguments: Some(args),
        })
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    holon_mcp_client::mcp_call_surface::extract_tool_response(&result)
}

fn response_object(value: &serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
    value.as_object().cloned().expect("response is an object")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_served_list_projects_shopping_items() {
    let base = start_mock(Mode::FullList).await;
    let surface = surface_for(&base);

    let response = list_items(&surface)
        .await
        .expect("the peer serves the list");
    let snapshot =
        CompleteSnapshot::from_response(&response_object(&response), FETCHED_AT).expect("snapshot");

    // Six wire rows, five keys: the two `Milk`/`R` rows are one item.
    assert_eq!(
        snapshot.len(),
        5,
        "duplicate (name, cat) rows were not folded"
    );

    let inserts: Vec<_> = ShoppingReconciler::default()
        .reconcile(&[], &snapshot)
        .expect("reconcile against an empty local list")
        .local
        .into_iter()
        .filter_map(|i| match i {
            LocalIntent::Insert(row) => Some(row),
            other => panic!("an empty local list can only take inserts, got {other:?}"),
        })
        .collect();
    assert_eq!(inserts.len(), 5);

    let milk = inserts
        .iter()
        .find(|r| r.name == "Milk" && r.category.as_wire() == "R")
        .expect("Milk/R row");
    // A row with no count still counts for one, so folding cannot lose a unit.
    assert_eq!(milk.count, Some(3.0));
    assert_eq!(
        milk.category.code(),
        &CategoryCode::Known(KnownCategory::R),
        "the category came through as a parsed code, not a string"
    );
    assert_eq!(milk.last_seen_remote.as_deref(), Some(FETCHED_AT));
    assert!(!milk.checked, "this endpoint carries no checked state");
    assert_eq!(milk.product_id, None);
    assert_eq!(milk.deleted_at, None);

    // Same name, different aisle: two items, not a collision.
    assert!(
        inserts
            .iter()
            .any(|r| r.name == "Milk" && r.category.as_wire() == "Ca"),
        "Milk/Ca collapsed into Milk/R"
    );

    // A code outside this build's vocabulary is carried verbatim and marked,
    // never mapped onto a neighbouring aisle and never dropped.
    let salmon = inserts.iter().find(|r| r.name == "Salmon").expect("Salmon");
    assert_eq!(
        salmon.category.code(),
        &CategoryCode::Unrecognized("Fish".to_string())
    );
    assert!(!salmon.category.is_recognized());

    // A `<code>_<qualifier>` value keeps both halves, so it round-trips.
    let screws = inserts.iter().find(|r| r.name == "Screws").expect("Screws");
    assert_eq!(
        screws.category.code(),
        &CategoryCode::Known(KnownCategory::Ir)
    );
    assert_eq!(screws.category.qualifier(), Some("shed"));
    assert_eq!(screws.category.as_wire(), "Ir_shed");
}

#[tokio::test]
async fn a_truncated_response_changes_nothing() {
    let base = start_mock(Mode::TruncatedBody).await;
    let surface = surface_for(&base);

    let err = list_items(&surface)
        .await
        .expect_err("a body that stops mid-JSON must not be accepted as a list");
    // The failure is loud and stops here: no snapshot exists, so the reconciler
    // it feeds is never reached and no row can be deleted by absence.
    assert!(
        format!("{err:#}").contains("not JSON"),
        "the truncation was not reported as a body-parse failure: {err:#}"
    );
}

#[tokio::test]
async fn a_failing_response_changes_nothing() {
    let base = start_mock(Mode::EchoUrlIn500).await;
    let surface = surface_for(&base);

    let err = list_items(&surface)
        .await
        .expect_err("a 500 must not be accepted as an empty list");
    assert!(
        format!("{err:#}").contains("500"),
        "the failure did not name the status: {err:#}"
    );
}

#[tokio::test]
async fn a_capability_token_in_the_url_path_never_reaches_an_error() {
    let base = start_mock(Mode::EchoUrlIn500).await;
    let surface = surface_for(&base);

    let err = format!(
        "{:#}",
        list_items(&surface)
            .await
            .expect_err("the mock answers 500")
    );
    // The token sits in a PATH segment and the upstream echoed the whole path
    // back, so both the request URL and the response body carry it.
    assert!(
        !err.contains(CAP_TOKEN),
        "the capability token leaked into an error string: {err}"
    );
    // ...and the message still says where the secret stood, rather than having
    // gone quiet about the request.
    assert!(
        err.contains("<redacted>"),
        "the error redacted nothing visibly: {err}"
    );
}

#[test]
fn the_shipped_sidecar_holds_no_resolved_url() {
    assert!(
        SHOPPING_SIDECAR.contains("base_url: ${SHOPPING_LIST_URL}"),
        "the shopping sidecar must reference its capability URL as a variable"
    );
    // A resolved capability URL in the repo would be the credential itself.
    assert!(
        !SHOPPING_SIDECAR.contains("https://"),
        "the shopping sidecar carries a literal URL"
    );
}

#[test]
fn the_shipped_sidecar_declares_no_mirrored_entity() {
    let cfg: IntegrationFileConfig =
        serde_yaml::from_str(SHOPPING_SIDECAR).expect("the shipped shopping sidecar parses");
    // The generic entity mirror keys rows on a server-issued id column and
    // fails loud without one. This peer issues none, so an entity here would
    // be a sidecar that breaks the moment someone enables it.
    assert!(
        cfg.entities.is_empty(),
        "the shopping sidecar declares an entity the id-less wire shape cannot mirror"
    );
}

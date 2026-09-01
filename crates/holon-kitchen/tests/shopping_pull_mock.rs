//! The shopping peer end-to-end, over the REAL `rest` transport against a LOCAL
//! mock HTTP server (no network), driving the SHIPPED sidecar.
//!
//! The mock is a stateful list, not a canned body: it applies the commands a
//! commit sends and versions itself, so "both peers mutated the same list
//! between polls" is a scenario the test can actually stage. What is under
//! test: that a served list becomes `shopping_item` intents, that an incomplete
//! fetch changes NOTHING (absence is only a deletion signal inside a complete
//! snapshot), that a local addition reaches the peer and the round converges,
//! that a stale version re-pulls instead of overwriting, and that the
//! credential in the URL path never reaches an error string.
//!
//! The mock is hand-rolled HTTP/1.1 on a `TcpListener`, mirroring
//! `crates/holon-mcp-client/tests/rest_transport_mock.rs`, so the test pulls in
//! no HTTP-server dependency.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::Ordering::SeqCst;

use anyhow::Result;
use async_trait::async_trait;
use holon_kitchen::shopping::ItemKey;
use holon_kitchen::shopping::ListVersion;
use holon_kitchen::shopping::LocalIntent;
use holon_kitchen::shopping::LocalShoppingItem;
use holon_kitchen::shopping::PushIntent;
use holon_kitchen::shopping::ShoppingCategory;
use holon_kitchen::shopping::ShoppingReconciler;
use holon_kitchen::shopping_rest::RestShoppingPeer;
use holon_kitchen::shopping_sync::CommitBatch;
use holon_kitchen::shopping_sync::ShoppingPeer;
use holon_kitchen::shopping_sync::ShoppingRowReader;
use holon_kitchen::shopping_sync::local_intent_operation;
use holon_kitchen::shopping_sync::sync_once;
use holon_mcp_client::CredentialRoot;
use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use holon_mcp_client::rest_transport::RestCallSurface;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// The credential: an opaque `!`-marked token this endpoint carries in a URL
/// PATH segment, stable per list. Synthetic — a captured token never belongs in
/// a fixture.
const CAP_TOKEN: &str = "!cap7f3a9d2e4b8c1056xyzQ3rT7vB2n";
const DEVICE_ID: &str = "device-under-test";

/// The bundled sidecar under test.
const SHOPPING_SIDECAR: &str = include_str!("../../../assets/integrations/shopping.yaml");

/// The vocabulary the mock list publishes. `Kleidung_clothes_1976D2` carries
/// the icon/colour decoration the capture recorded.
const CATS: &[&str] = &["R", "B", "Ca", "Ir", "Kleidung_clothes_1976D2"];

// ---------------------------------------------------------------------------
// Mock peer — a stateful list
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// A well-formed list that applies commits and versions itself.
    Live,
    /// 200, but the body stops mid-JSON — a fetch that "succeeded" and is not
    /// complete.
    TruncatedBody,
    /// The upstream failed and echoed the request URL back at us.
    EchoUrlIn500,
    /// The first commit is rejected: the peer answers the version it already
    /// had, applies nothing, and someone else's write is visible on the next
    /// pull.
    StaleFirstCommit,
}

#[derive(Default)]
struct ListState {
    /// `(name, cat, count)` — the ACTIVE list.
    items: Vec<(String, String, Option<f64>)>,
    /// `(name, cat)` — the checked-off ones.
    picked: Vec<(String, String)>,
    version: i64,
    commits: usize,
}

impl ListState {
    fn body(&self) -> String {
        let items: Vec<serde_json::Value> = self
            .items
            .iter()
            .map(|(name, cat, count)| match count {
                Some(c) => serde_json::json!({"name": name, "cat": cat, "count": c}),
                None => serde_json::json!({"name": name, "cat": cat}),
            })
            .collect();
        let picked: serde_json::Map<String, serde_json::Value> = self
            .picked
            .iter()
            .map(|(name, cat)| {
                (
                    name.clone(),
                    serde_json::json!({"cat": cat, "date": "2026-09-01T08:00:00Z"}),
                )
            })
            .collect();
        serde_json::json!({
            "items": items,
            "pickedItems": picked,
            "version": self.version,
            "options": {"prices": false, "cats": CATS},
        })
        .to_string()
    }

    fn apply(&mut self, commands: &[serde_json::Value]) {
        for command in commands {
            let good = &command["good"];
            let name = good["name"].as_str().unwrap_or_default().to_string();
            let cat = good["cat"].as_str().unwrap_or_default().to_string();
            match command["cmd"].as_str().unwrap_or_default() {
                "add" => self.items.push((name, cat, None)),
                "del" => {
                    self.items.retain(|(n, c, _)| !(n == &name && c == &cat));
                    self.picked.retain(|(n, c)| !(n == &name && c == &cat));
                }
                other => panic!("the mock peer received an unknown command '{other}'"),
            }
        }
    }
}

struct Mock {
    base_url: String,
    state: Arc<Mutex<ListState>>,
}

fn seeded_state(version: i64) -> ListState {
    ListState {
        items: vec![
            ("Milk".into(), "R".into(), Some(2.0)),
            ("Milk".into(), "R".into(), None),
            ("Milk".into(), "Ca".into(), Some(1.0)),
            ("Bread".into(), "B".into(), None),
            ("Salmon".into(), "Fish".into(), Some(1.0)),
            ("Socks".into(), "Kleidung".into(), None),
        ],
        picked: vec![("Bread".into(), "B".into())],
        version,
        commits: 0,
    }
}

async fn start_mock(mode: Mode) -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let state = Arc::new(Mutex::new(seeded_state(7)));
    let state_bg = state.clone();
    let stale_used = Arc::new(AtomicBool::new(false));

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let state_conn = state_bg.clone();
            let stale_used = stale_used.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
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
                let path = request_line.next().unwrap_or_default().to_string();
                let request_body =
                    String::from_utf8_lossy(&buf[head_end..head_end + content_length]).to_string();

                let (status, body) = if mode == Mode::EchoUrlIn500 {
                    (
                        "500 Internal Server Error",
                        format!("{{\"error\":\"upstream failed for {path}\"}}"),
                    )
                } else if method == "POST" {
                    let sent: serde_json::Value =
                        serde_json::from_str(&request_body).expect("the commit body is JSON");
                    let commands = sent["commands"]
                        .as_array()
                        .expect("the commit carries a commands array")
                        .clone();
                    let mut state = state_conn.lock().expect("mock list");
                    state.commits += 1;
                    if mode == Mode::StaleFirstCommit && !stale_used.swap(true, SeqCst) {
                        // Rejected: nothing applied, the version stands, and
                        // someone else's item is now on the list.
                        state.items.push(("Yeast".into(), "B".into(), None));
                        state.version += 1;
                    } else {
                        state.apply(&commands);
                        state.version += 1;
                    }
                    (
                        "200 OK",
                        serde_json::json!({
                            "version": state.version,
                            "pickedItemsVersion": state.version,
                            "options": {"prices": false},
                        })
                        .to_string(),
                    )
                } else {
                    let body = state_conn.lock().expect("mock list").body();
                    match mode {
                        Mode::TruncatedBody => ("200 OK", body[..body.len() / 2].to_string()),
                        _ => ("200 OK", body),
                    }
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

    Mock {
        // The share link of ONE list: host, credential segment, and the list
        // itself. Both calls are relative paths off it.
        base_url: format!("http://{addr}/{CAP_TOKEN}/api/list/l-42"),
        state,
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// Drive the SHIPPED sidecar, with its one `${VAR}` pointed at the mock. The
/// whole base URL is the credential here, so the whole base URL is the
/// registered secret.
fn surface_for(base_url: &str) -> Arc<dyn McpCallSurface> {
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
            &CredentialRoot::new("/tmp/holon-shopping-c2-config"),
        )
        .expect("shopping sidecar resolves into an mcp config");
    match mcp.transport {
        McpTransport::Rest { manual, .. } => Arc::new(RestCallSurface::new(manual)),
        other => panic!("expected the rest transport, got {other:?}"),
    }
}

fn peer_for(base_url: &str) -> RestShoppingPeer {
    RestShoppingPeer::new(surface_for(base_url), DEVICE_ID)
}

/// The local rows a round starts from.
struct Rows(Vec<LocalShoppingItem>);

#[async_trait]
impl ShoppingRowReader for Rows {
    async fn load(&self) -> Result<Vec<LocalShoppingItem>> {
        Ok(self.0.clone())
    }
}

fn local(name: &str, cat: &str, count: Option<f64>) -> LocalShoppingItem {
    let category = ShoppingCategory::unresolved(cat);
    LocalShoppingItem {
        id: ItemKey::new(name, &category).row_id(),
        name: name.to_string(),
        category,
        count,
        checked: false,
        product_id: None,
        deleted_at: None,
        last_seen_remote: Some("2026-08-31T10:00:00Z".into()),
    }
}

// ---------------------------------------------------------------------------
// Pull
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_served_list_projects_shopping_items() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_for(&mock.base_url);

    let snapshot = peer.pull().await.expect("the peer serves the list");

    // Six wire rows, five keys: the two `Milk`/`R` rows are one item.
    assert_eq!(
        snapshot.len(),
        5,
        "duplicate (name, cat) rows were not folded"
    );
    assert_eq!(snapshot.version().list, 7, "the list version came through");
    assert_eq!(
        snapshot.vocabulary().len(),
        CATS.len(),
        "the vocabulary came from the list's own options.cats"
    );

    let inserts: Vec<_> = ShoppingReconciler::default()
        .reconcile(&[], &snapshot)
        .expect("reconcile against an empty local list")
        .local
        .into_iter()
        .map(|i| match i {
            LocalIntent::Insert(row) => row,
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
    assert!(milk.category.is_recognized());
    // The peer stamps the fetch time; its VALUE is pinned in the reconciler
    // tests, which supply one. Here only its presence is the watermark claim.
    assert!(milk.last_seen_remote.is_some());
    assert!(!milk.checked);

    // Same name, different aisle: two items, not a collision.
    assert!(
        inserts
            .iter()
            .any(|r| r.name == "Milk" && r.category.as_wire() == "Ca"),
        "Milk/Ca collapsed into Milk/R"
    );

    // `pickedItems` membership IS the checked flag.
    let bread = inserts.iter().find(|r| r.name == "Bread").expect("Bread");
    assert!(bread.checked, "a checked-off item arrived unchecked");

    // A code the list did not publish is carried verbatim and marked, never
    // mapped onto a neighbouring aisle and never dropped.
    let salmon = inserts.iter().find(|r| r.name == "Salmon").expect("Salmon");
    assert_eq!(salmon.category.as_wire(), "Fish");
    assert!(!salmon.category.is_recognized());

    // A decorated vocabulary entry resolves for the plain code an item carries.
    let socks = inserts.iter().find(|r| r.name == "Socks").expect("Socks");
    assert!(socks.category.is_recognized());
    assert_eq!(
        socks.category.entry().and_then(|e| e.color()),
        Some("1976D2")
    );
}

#[tokio::test]
async fn a_truncated_response_changes_nothing() {
    let mock = start_mock(Mode::TruncatedBody).await;
    let peer = peer_for(&mock.base_url);

    let err = peer
        .pull()
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
    let mock = start_mock(Mode::EchoUrlIn500).await;
    let peer = peer_for(&mock.base_url);

    let err = peer
        .pull()
        .await
        .expect_err("a 500 must not be accepted as an empty list");
    assert!(
        format!("{err:#}").contains("500"),
        "the failure did not name the status: {err:#}"
    );
}

#[tokio::test]
async fn a_capability_token_in_the_url_path_never_reaches_an_error() {
    let mock = start_mock(Mode::EchoUrlIn500).await;
    let peer = peer_for(&mock.base_url);

    let err = format!("{:#}", peer.pull().await.expect_err("the mock answers 500"));
    // The token sits in a PATH segment and the upstream echoed the whole path
    // back, so both the request URL and the response body carry it.
    assert!(
        !err.contains(CAP_TOKEN),
        "the capability token leaked into an error string: {err}"
    );
    assert!(
        err.contains("<redacted>"),
        "the error redacted nothing visibly: {err}"
    );
}

#[tokio::test]
async fn the_commit_leg_hides_the_credential_too() {
    // The write leg builds its own URL and sends a body; an upstream that
    // echoes the path fails exactly as loudly as on the read leg, and must
    // redact exactly as much. Defense in depth: the whole base URL is a
    // registered `${VAR}` secret AND the `!`-marked segment is scrubbed
    // structurally, so neither layer alone is load-bearing here.
    let mock = start_mock(Mode::EchoUrlIn500).await;
    let peer = peer_for(&mock.base_url);

    let batch = CommitBatch::from_push_intents(
        &[PushIntent::Add(local("Oat milk", "R", Some(1.0)))],
        ListVersion { list: 7, picked: 7 },
        DEVICE_ID,
        1_756_700_000_000,
    );
    let err = format!(
        "{:#}",
        peer.commit(&batch).await.expect_err("the mock answers 500")
    );
    assert!(
        !err.contains(CAP_TOKEN),
        "the capability token leaked from the write leg: {err}"
    );
    assert!(
        err.contains("<redacted>"),
        "the error redacted nothing: {err}"
    );
    assert!(
        err.contains("500"),
        "the failure did not name the status: {err}"
    );
}

// ---------------------------------------------------------------------------
// Push and convergence
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_local_addition_reaches_the_peer_and_the_round_converges() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_for(&mock.base_url);

    let mut mine = local("Oat milk", "R", Some(1.0));
    mine.last_seen_remote = None;
    let rows = Rows(vec![mine]);

    let outcome = sync_once(
        &peer,
        &rows,
        &ShoppingReconciler::default(),
        DEVICE_ID,
        1_756_700_000_000,
    )
    .await
    .expect("one round");

    assert_eq!(outcome.committed, 1, "the addition was not committed");
    assert!(!outcome.retried);
    assert!(
        mock.state
            .lock()
            .expect("mock list")
            .items
            .iter()
            .any(|(n, c, _)| n == "Oat milk" && c == "R"),
        "the peer never received the addition"
    );

    // A second round over the SAME local rows finds nothing left to push: the
    // item the first round sent now comes back in the snapshot.
    let again = sync_once(
        &peer,
        &rows,
        &ShoppingReconciler::default(),
        DEVICE_ID,
        1_756_700_001_000,
    )
    .await
    .expect("second round");
    assert_eq!(again.committed, 0, "the round did not converge");
}

#[tokio::test]
async fn a_local_deletion_reaches_the_peer_as_a_del_command() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_for(&mock.base_url);

    let mut gone = local("Bread", "B", None);
    gone.deleted_at = Some("2026-09-01T09:00:00Z".into());

    let outcome = sync_once(
        &peer,
        &Rows(vec![gone]),
        &ShoppingReconciler::default(),
        DEVICE_ID,
        1_756_700_000_000,
    )
    .await
    .expect("one round");

    assert_eq!(outcome.committed, 1);
    assert!(
        !mock
            .state
            .lock()
            .expect("mock list")
            .items
            .iter()
            .any(|(n, c, _)| n == "Bread" && c == "B"),
        "the peer still lists a locally deleted item"
    );
}

#[tokio::test]
async fn a_stale_version_re_pulls_instead_of_overwriting() {
    let mock = start_mock(Mode::StaleFirstCommit).await;
    let peer = peer_for(&mock.base_url);

    let mut mine = local("Oat milk", "R", Some(1.0));
    mine.last_seen_remote = None;

    let outcome = sync_once(
        &peer,
        &Rows(vec![mine]),
        &ShoppingReconciler::default(),
        DEVICE_ID,
        1_756_700_000_000,
    )
    .await
    .expect("the round recovers from a stale version");

    assert!(outcome.retried, "the conflict was not detected");
    // Two commands sent across two commits — the first was refused, the second
    // landed. The count is what was SENT, not what stuck.
    assert_eq!(outcome.committed, 2, "the retry did not commit");

    let state = mock.state.lock().expect("mock list");
    assert_eq!(state.commits, 2, "the round did not re-commit exactly once");
    // The concurrent writer's item survived — the retry re-pulled rather than
    // replaying the batch over a list it had not read.
    assert!(
        state.items.iter().any(|(n, _, _)| n == "Yeast"),
        "the concurrent write was overwritten"
    );
    assert!(
        state.items.iter().any(|(n, _, _)| n == "Oat milk"),
        "the retry lost our own addition"
    );
    // ...and it arrives in the local intents too, so the local rows converge on
    // the same list the peer holds.
    assert!(
        outcome.local.iter().any(|i| matches!(
            i,
            LocalIntent::Insert(row) if row.name == "Yeast"
        )),
        "the concurrent write never reached the local intents: {:?}",
        outcome.local
    );
}

#[tokio::test]
async fn every_local_write_goes_through_the_generic_operation_path() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_for(&mock.base_url);

    let outcome = sync_once(
        &peer,
        &Rows(vec![local("Bread", "B", None)]),
        &ShoppingReconciler::default(),
        DEVICE_ID,
        1_756_700_000_000,
    )
    .await
    .expect("one round");

    assert!(!outcome.local.is_empty());
    for intent in &outcome.local {
        let operation = local_intent_operation(intent);
        assert_eq!(
            operation.entity_name.as_str(),
            "shopping-item",
            "an intent addressed something other than the declared type"
        );
        assert!(
            matches!(
                operation.op_name.as_str(),
                "create" | "set_field" | "delete"
            ),
            "the sync minted its own write op '{}' instead of the type's generic authority",
            operation.op_name
        );
        assert!(
            operation.params.contains_key("id"),
            "a write with no row id: {operation:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// The shipped sidecar
// ---------------------------------------------------------------------------

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

//! The GENERIC remote-list peer end-to-end, over the REAL `rest` transport
//! against a LOCAL mock HTTP server (no network), driving the SHIPPED shopping
//! sidecar. Nothing under test here knows it is a shopping list: the connection
//! is whatever `holon.list_sync` declares.
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
use holon_api::Value;
use holon_app::remote_list::RestListPeer;
use holon_connections::CommitBatch;
use holon_connections::CompiledListSync;
use holon_connections::ListSnapshot;
use holon_connections::ListSyncSpec;
use holon_connections::LocalIntent;
use holon_connections::LocalRow;
use holon_connections::LocalRowReader;
use holon_connections::PushIntent;
use holon_connections::RemoteListPeer;
use holon_connections::RemoteListReconciler;
use holon_connections::local_intent_operation;
use holon_connections::sync_once;
use holon_core::file_format::TypedRowSet;
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
    /// Every request path the mock served, in order. The pull's freshness
    /// argument is only observable on the wire, so this is where it is read.
    paths: Vec<String>,
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
        paths: Vec::new(),
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
                state_conn
                    .lock()
                    .expect("mock list")
                    .paths
                    .push(path.clone());

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

/// The connection the shipped sidecar declares, compiled against the shipped
/// `shopping_item` type. Both halves come from the assets: a test that restated
/// either would stop being a test of this connection.
fn connection() -> Arc<CompiledListSync> {
    let doc: serde_yaml::Value =
        serde_yaml::from_str(SHOPPING_SIDECAR).expect("the shipped shopping sidecar parses");
    let spec: ListSyncSpec = serde_yaml::from_value(doc["holon"]["list_sync"].clone())
        .expect("the sidecar declares a list_sync block");
    let declared = holon_kitchen::shopping_item_type().expect("the declared type");
    CompiledListSync::compile("shopping", spec, &declared).expect("the connection compiles")
}

fn peer_for(base_url: &str) -> RestListPeer {
    RestListPeer::new(surface_for(base_url), connection(), DEVICE_ID)
}

/// A version envelope with no items, for a leg that needs a batch to send
/// rather than a list to reconcile.
fn empty_snapshot(compiled: &CompiledListSync, version: i64) -> ListSnapshot {
    let spec = compiled.spec();
    let mut list_row: holon_api::entity::StorageEntity = Default::default();
    list_row.insert("id".into(), Value::String("shopping".into()));
    list_row.insert("version".into(), Value::Integer(version));
    list_row.insert("picked_items_version".into(), Value::Integer(version));
    let sets = vec![TypedRowSet {
        type_name: spec.list_row_type.clone(),
        owner_column: "list".into(),
        owner_value: "shopping".into(),
        rows: vec![list_row],
    }];
    ListSnapshot::from_rows(compiled, &sets, chrono::Utc::now().to_rfc3339())
        .expect("an empty list is still a complete snapshot")
}

/// The local rows a round starts from.
struct Rows(Vec<LocalRow>);

#[async_trait]
impl LocalRowReader for Rows {
    async fn load(&self) -> Result<Vec<LocalRow>> {
        Ok(self.0.clone())
    }
}

/// One segment of a row id, escaped the way the sidecar's `@uri` does. An id is
/// a reference and must parse as a URI, and an item name is free text: spaces
/// and umlauts are ordinary in a shopping list.
fn uri_segment(raw: &str) -> String {
    raw.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            other => format!("%{other:02X}"),
        })
        .collect()
}

/// One mirror row, spelled in the declared type's columns. The id is derived
/// from the content pair because this peer issues none — which is exactly what
/// the connection's `key` expression says.
fn local(name: &str, cat: &str, count: Option<f64>) -> LocalRow {
    let id = format!("shopping-item:{}:{}", uri_segment(cat), uri_segment(name));
    let mut columns = std::collections::BTreeMap::new();
    columns.insert("id".to_string(), Value::String(id.clone()));
    columns.insert("name".to_string(), Value::String(name.to_string()));
    columns.insert("cat".to_string(), Value::String(cat.to_string()));
    columns.insert(
        "count".to_string(),
        count.map(Value::Float).unwrap_or(Value::Null),
    );
    columns.insert("checked".to_string(), Value::Integer(0));
    columns.insert("deleted_at".to_string(), Value::Null);
    columns.insert(
        "last_seen_remote".to_string(),
        Value::String("2026-08-31T10:00:00Z".into()),
    );
    LocalRow { id, columns }
}

fn set(row: &mut LocalRow, column: &str, value: Value) {
    row.columns.insert(column.to_string(), value);
}

// ---------------------------------------------------------------------------
// Pull
// ---------------------------------------------------------------------------

#[tokio::test]
async fn a_served_list_projects_shopping_items() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_for(&mock.base_url);
    let compiled = connection();

    let snapshot = peer.pull().await.expect("the peer serves the list");

    // Six wire rows, five keys: the two `Milk`/`R` rows are one item. The FOLD
    // happens in the sidecar's own mapping; what the generic path adds is that
    // a pair which survived the fold and still collided would be refused.
    assert_eq!(
        snapshot.len(),
        5,
        "duplicate (name, cat) rows were not folded"
    );
    assert_eq!(snapshot.version(), 7, "the list version came through");
    // The category vocabulary is a SECOND declared type the same mapping emits.
    // It never enters the list snapshot, and its content is pinned where the
    // mapping is: `crates/holon-kitchen/tests/shopping_mapping_differential.rs`.

    let inserts: Vec<_> = RemoteListReconciler::new(compiled.clone())
        .reconcile(&[], &snapshot)
        .expect("reconcile against an empty local list")
        .local
        .into_iter()
        .map(|intent| match intent {
            LocalIntent::Insert { id, columns } => (id, columns),
            other => panic!("an empty local list can only take inserts, got {other:?}"),
        })
        .collect();
    assert_eq!(inserts.len(), 5);

    let column = |id: &str, name: &str| -> Option<Value> {
        inserts
            .iter()
            .find(|(row_id, _)| row_id == id)
            .and_then(|(_, columns)| columns.get(name).cloned())
    };

    let milk = "shopping-item:R:Milk";
    assert_eq!(column(milk, "name"), Some(Value::String("Milk".into())));
    // A row with no count still counts for one, so folding cannot lose a unit.
    // Read as a number rather than a spelling: the peer's JSON and the REAL
    // column disagree on Integer-vs-Float and mean the same thing.
    assert_eq!(
        match column(milk, "count") {
            Some(Value::Integer(n)) => n as f64,
            Some(Value::Float(f)) => f,
            other => panic!("`count` is not a number (got {other:?})"),
        },
        3.0
    );
    // The peer stamps the fetch time; its VALUE is pinned in the reconciler
    // tests, which supply one. Here only its presence is the watermark claim.
    assert!(
        matches!(column(milk, "last_seen_remote"), Some(Value::String(_))),
        "the insert carries no watermark"
    );
    assert_eq!(column(milk, "checked"), Some(Value::Boolean(false)));

    // Same name, different aisle: two items, not a collision.
    assert!(
        column("shopping-item:Ca:Milk", "name").is_some(),
        "Milk/Ca collapsed into Milk/R"
    );

    // `pickedItems` membership IS the checked flag.
    assert_eq!(
        column("shopping-item:B:Bread", "checked"),
        Some(Value::Boolean(true)),
        "a checked-off item arrived unchecked"
    );

    // A code the list did not publish is carried verbatim, never mapped onto a
    // neighbouring aisle and never dropped.
    assert_eq!(
        column("shopping-item:Fish:Salmon", "cat"),
        Some(Value::String("Fish".into()))
    );

    // An insert carries the peer's columns the declared type HAS, and nothing
    // else: an undeclared column would land in the overflow bag as a property
    // nobody declared.
    let (_, socks) = inserts
        .iter()
        .find(|(id, _)| id == "shopping-item:Kleidung:Socks")
        .expect("Socks");
    for column in socks.keys() {
        assert!(
            compiled.declares_column(column),
            "the insert carries the undeclared column '{column}'"
        );
    }
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
    let compiled = connection();

    let row = local("Oat milk", "R", Some(1.0));
    let key = compiled
        .key_of(&serde_json::json!({ "name": "Oat milk", "cat": "R" }))
        .expect("the connection derives the key");
    let batch = CommitBatch::from_push_intents(
        &[PushIntent::Add { key, row }],
        &empty_snapshot(&compiled, 7),
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
    let reconciler = RemoteListReconciler::new(connection());

    let mut mine = local("Oat milk", "R", Some(1.0));
    set(&mut mine, "last_seen_remote", Value::Null);
    let rows = Rows(vec![mine]);

    let outcome = sync_once(&peer, &rows, &reconciler, DEVICE_ID, 1_756_700_000_000)
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
    let again = sync_once(&peer, &rows, &reconciler, DEVICE_ID, 1_756_700_001_000)
        .await
        .expect("second round");
    assert_eq!(again.committed, 0, "the round did not converge");
}

#[tokio::test]
async fn a_local_deletion_reaches_the_peer_as_a_del_command() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_for(&mock.base_url);
    let compiled = connection();

    let mut gone = local("Bread", "B", None);
    // The peer stamps the snapshot with the wall clock, and the reconciler
    // measures the tombstone against THAT — so "still live" has to be written
    // relative to now, or the fixture ages out of the window on a calendar date
    // and the test stops exercising the push leg.
    let half_window = compiled.tombstone_window() / 2;
    set(
        &mut gone,
        "deleted_at",
        Value::String((chrono::Utc::now() - half_window).to_rfc3339()),
    );

    let outcome = sync_once(
        &peer,
        &Rows(vec![gone]),
        &RemoteListReconciler::new(compiled),
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
    set(&mut mine, "last_seen_remote", Value::Null);

    let outcome = sync_once(
        &peer,
        &Rows(vec![mine]),
        &RemoteListReconciler::new(connection()),
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
        outcome.local.iter().any(|intent| matches!(
            intent,
            LocalIntent::Insert { columns, .. }
                if columns.get("name") == Some(&Value::String("Yeast".into()))
        )),
        "the concurrent write never reached the local intents: {:?}",
        outcome.local
    );
}

#[tokio::test]
async fn every_local_write_goes_through_the_generic_operation_path() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_for(&mock.base_url);
    let compiled = connection();

    let outcome = sync_once(
        &peer,
        &Rows(vec![local("Bread", "B", None)]),
        &RemoteListReconciler::new(compiled.clone()),
        DEVICE_ID,
        1_756_700_000_000,
    )
    .await
    .expect("one round");

    assert!(!outcome.local.is_empty());
    for intent in &outcome.local {
        let operation = local_intent_operation(compiled.spec(), intent);
        assert_eq!(
            operation.entity_name.as_str(),
            "shopping-item",
            "an intent addressed something other than the declared type"
        );
        assert!(
            matches!(operation.op_name.as_str(), "create" | "set_field" | "purge"),
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
    // A UTCP manual states an absolute `url` per tool, so the share link
    // appears once per call rather than once as a shared base.
    assert!(
        SHOPPING_SIDECAR.matches("${SHOPPING_LIST_URL}").count() >= 2,
        "both of the shopping manual's tools must reference the capability URL as a variable"
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
    // The generic ENTITY mirror keys rows on a server-issued id column and
    // fails loud without one. This peer issues none — which is why it is a
    // `list_sync` connection, whose identity is the declared key expression.
    assert!(
        cfg.entities.is_empty(),
        "the shopping sidecar declares an entity the id-less wire shape cannot mirror"
    );
}

// ---------------------------------------------------------------------------
// Freshness is DECLARED
// ---------------------------------------------------------------------------

/// The shipped sidecar with every trace of its freshness argument removed — a
/// connection that declares no cache buster, which is the default and the
/// common case. Edited structurally rather than by line, because the argument
/// appears in three places: the manual's input schema, the query template, and
/// the `list_sync` declaration.
fn sidecar_without_a_cache_buster() -> String {
    let mut doc: serde_yaml::Value =
        serde_yaml::from_str(SHOPPING_SIDECAR).expect("the shipped sidecar parses");

    let list_sync = doc["holon"]["list_sync"]
        .as_mapping_mut()
        .expect("the sidecar declares a list_sync block");
    assert!(
        list_sync
            .remove(serde_yaml::Value::from("cache_buster"))
            .is_some(),
        "the shipped sidecar declares no cache_buster, so this fixture removes nothing"
    );

    doc["holon"]["tools"]["pull_list"]["query"]
        .as_mapping_mut()
        .expect("the pull tool declares a query")
        .remove(serde_yaml::Value::from("_nocache"));

    let inputs = &mut doc["utcp"]["tools"][0]["inputs"];
    inputs["properties"]
        .as_mapping_mut()
        .expect("the manual declares the pull's inputs")
        .remove(serde_yaml::Value::from("_nocache"));
    let required = inputs["required"]
        .as_sequence_mut()
        .expect("the manual declares which inputs are required");
    required.retain(|name| name.as_str() != Some("_nocache"));

    serde_yaml::to_string(&doc).expect("the edited sidecar serializes")
}

fn peer_from(sidecar: &str, base_url: &str) -> RestListPeer {
    let cfg: IntegrationFileConfig = serde_yaml::from_str(sidecar).expect("the sidecar parses");
    let lookup = |name: &str| match name {
        "SHOPPING_LIST_URL" => Some(base_url.to_string()),
        _ => None,
    };
    let mcp = cfg
        .into_mcp_config_with(
            "shopping".to_string(),
            &lookup,
            &CredentialRoot::new("/tmp/holon-shopping-c2-config"),
        )
        .expect("the sidecar resolves into an mcp config");
    let surface: Arc<dyn McpCallSurface> = match mcp.transport {
        McpTransport::Rest { manual, .. } => Arc::new(RestCallSurface::new(manual)),
        other => panic!("expected the rest transport, got {other:?}"),
    };
    let doc: serde_yaml::Value = serde_yaml::from_str(sidecar).expect("the sidecar parses");
    let spec: ListSyncSpec = serde_yaml::from_value(doc["holon"]["list_sync"].clone())
        .expect("the sidecar declares a list_sync block");
    let declared = holon_kitchen::shopping_item_type().expect("the declared type");
    let compiled =
        CompiledListSync::compile("shopping", spec, &declared).expect("the connection compiles");
    RestListPeer::new(surface, compiled, DEVICE_ID)
}

#[tokio::test]
async fn a_connection_that_declares_no_cache_buster_sends_none() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_from(&sidecar_without_a_cache_buster(), &mock.base_url);

    peer.pull().await.expect("the peer serves the list");

    let paths = mock.state.lock().expect("mock list").paths.clone();
    assert_eq!(paths.len(), 1, "one pull, one request");
    assert!(
        !paths[0].contains("nocache"),
        "a connection declaring no cache buster still sent one: {}",
        paths[0]
    );
}

#[tokio::test]
async fn the_shopping_connection_declares_epoch_millis_and_sends_it() {
    let mock = start_mock(Mode::Live).await;
    let peer = peer_for(&mock.base_url);

    peer.pull().await.expect("the peer serves the list");

    let paths = mock.state.lock().expect("mock list").paths.clone();
    let sent = paths[0]
        .split(['?', '&'])
        .find_map(|pair| pair.strip_prefix("_nocache="))
        .unwrap_or_else(|| panic!("the pull carried no freshness argument: {}", paths[0]));
    // Epoch milliseconds, so a whole number well past the epoch. The VALUE
    // being fresh is what the write leg's verifying re-read depends on.
    let millis: i64 = sent
        .parse()
        .expect("the freshness argument is a whole number");
    assert!(
        millis > 1_700_000_000_000,
        "not an epoch-millisecond value: {sent}"
    );
}

/// The generic argument builder is where "declared, not assumed" is decided:
/// the wire only shows the freshness value when the peer's own `query`
/// template ALSO names it, so a test reading the URL cannot tell a connection
/// that stopped sending one from a template that never placed it.
#[test]
fn the_pull_arguments_carry_a_freshness_value_only_when_one_is_declared() {
    let declared: ListSyncSpec =
        serde_yaml::from_value(
            serde_yaml::from_str::<serde_yaml::Value>(SHOPPING_SIDECAR)
                .expect("the sidecar parses")["holon"]["list_sync"]
                .clone(),
        )
        .expect("the shipped block");
    let args = holon_app::remote_list::pull_arguments(&declared, 7, DEVICE_ID);
    assert_eq!(args["version"], serde_json::json!(7));
    assert_eq!(args["device_id"], serde_json::json!(DEVICE_ID));
    let millis = args["nocache"]
        .as_i64()
        .expect("the declared freshness value is a whole number");
    assert!(
        millis > 1_700_000_000_000,
        "not epoch milliseconds: {millis}"
    );

    let undeclared: ListSyncSpec = serde_yaml::from_value(
        serde_yaml::from_str::<serde_yaml::Value>(&sidecar_without_a_cache_buster())
            .expect("the fixture parses")["holon"]["list_sync"]
            .clone(),
    )
    .expect("the fixture's block");
    let args = holon_app::remote_list::pull_arguments(&undeclared, 7, DEVICE_ID);
    assert!(
        !args.contains_key("nocache"),
        "a connection declaring no cache buster was still handed one: {args:?}"
    );
}

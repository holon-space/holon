//! A peer does not only answer what Holon asks: it also pushes
//! `notifications/resources/updated` whenever it likes, with a URI it chooses,
//! and nothing drains them until the boot org scan opens the sync gate. Both
//! the number of notices and the length of each URI are therefore memory the
//! PEER sizes.
//!
//! The bounds are its connection's budget, and neither of them may lose a
//! change signal — a lost one leaves rows stale while the UI shows them as
//! current — so a refused notice widens the work instead: one full re-sync,
//! announced.

use std::collections::HashMap;
use std::time::Duration;

use holon_mcp_client::MAX_NOTIFICATION_URI_BYTES;
use holon_mcp_client::MAX_PENDING_SYNC_URIS;
use holon_mcp_client::connect_mcp_child_with_handler;
use holon_mcp_client::peer_budget::PeerBudget;

/// How long the flood is given to arrive. The sidecar writes it as fast as the
/// pipe takes it.
const ARRIVAL: Duration = Duration::from_secs(10);

/// A sidecar that answers `initialize` and then sends `count`
/// `notifications/resources/updated`, each with a distinct URI of `size` bytes.
fn flooding_sidecar(count: usize, size: usize) -> (String, Vec<String>) {
    let script = r#"
import sys, json
count = int(sys.argv[1]); size = int(sys.argv[2])
def send(o):
    sys.stdout.write(json.dumps(o) + "\n"); sys.stdout.flush()
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    msg = json.loads(line)
    m = msg.get("method")
    if m == "initialize":
        send({"jsonrpc": "2.0", "id": msg["id"], "result": {
            "protocolVersion": "2025-03-26", "capabilities": {},
            "serverInfo": {"name": "flood", "version": "0"}}})
    elif m == "notifications/initialized":
        for i in range(count):
            send({"jsonrpc": "2.0", "method": "notifications/resources/updated",
                  "params": {"uri": "res://" + str(i) + "/" + "x" * size}})
    elif msg.get("id") is not None:
        send({"jsonrpc": "2.0", "id": msg["id"],
              "result": {"tools": []} if m == "tools/list" else {}})
"#;
    (
        "/usr/bin/python3".to_string(),
        vec![
            "-c".to_string(),
            script.to_string(),
            count.to_string(),
            size.to_string(),
        ],
    )
}

/// What the connection's inbound queue holds after the flood, and the reason a
/// bound gave for collapsing it.
async fn flood(count: usize, size: usize) -> (Vec<String>, Option<String>) {
    let (command, args) = flooding_sidecar(count, size);
    let budget = PeerBudget::new();
    let (handler, mut receiver) = budget.notifying_handler();
    let (_peer, _service) =
        connect_mcp_child_with_handler(&command, &args, &HashMap::new(), handler, budget)
            .await
            .expect("the sidecar connects");
    // Nothing drains the queue before the sync gate opens, which is the
    // production window this is about.
    tokio::time::sleep(ARRIVAL).await;
    let mut parked = Vec::new();
    while let Ok(uri) = receiver.uris.try_recv() {
        parked.push(uri);
    }
    let collapsed = receiver.notices.collapses().borrow().clone();
    (parked, collapsed)
}

/// 1024 notices, each with a 128 KiB URI: on the unbounded path this parked
/// 134 MB. A URI that long matches no resource, so none of them is worth
/// parking — and the peer is still telling us something changed.
#[tokio::test(flavor = "multi_thread")]
async fn notices_with_oversized_uris_are_refused_and_collapse_into_a_full_resync() {
    let (parked, collapsed) = flood(1024, 128 * 1024).await;
    assert!(
        parked.is_empty(),
        "no URI past MAX_NOTIFICATION_URI_BYTES may be parked; got {} of them",
        parked.len()
    );
    let reason = collapsed.expect(
        "a refused notice must announce itself — a change signal that silently vanishes leaves \
         rows stale while the UI shows them as current",
    );
    assert!(
        reason.contains("MAX_NOTIFICATION_URI_BYTES"),
        "the reason must name the bound it hit; got: {reason}"
    );
}

/// 1024 notices with ordinary URIs: each one fits, so the bound that holds is
/// how many may wait.
#[tokio::test(flavor = "multi_thread")]
async fn more_pending_notices_than_the_bound_collapse_into_a_full_resync() {
    let (parked, collapsed) = flood(1024, 64).await;
    assert!(
        parked.len() <= MAX_PENDING_SYNC_URIS,
        "at most MAX_PENDING_SYNC_URIS ({MAX_PENDING_SYNC_URIS}) URIs may wait; got {}",
        parked.len()
    );
    assert!(
        parked
            .iter()
            .all(|uri| uri.len() <= MAX_NOTIFICATION_URI_BYTES),
        "every parked URI is within MAX_NOTIFICATION_URI_BYTES"
    );
    let reason = collapsed.expect("the notices that did not fit must announce themselves");
    assert!(
        reason.contains("MAX_PENDING_SYNC_URIS"),
        "the reason must name the bound it hit; got: {reason}"
    );
}

/// One stdio message of 68 MiB. rmcp's line codec has no maximum length and
/// `TokioChildProcess` gives no way to set one, so the cap lives in Holon's own
/// child transport, charged to the same per-connection allowance as the HTTP
/// legs' partial events.
///
/// The observable is the CONNECTION, not the notice: a sidecar framing a
/// message that large is past what this transport reads, so the leg ends and
/// every later call to it fails. ("Nothing was delivered" alone would also be
/// true of a URI the inbound bound refused.)
#[tokio::test(flavor = "multi_thread")]
async fn one_stdio_message_past_the_byte_allowance_ends_the_connection() {
    let (command, args) = flooding_sidecar(1, 68 * 1024 * 1024);
    let budget = PeerBudget::new();
    let (handler, _receiver) = budget.notifying_handler();
    let (peer, _service) =
        connect_mcp_child_with_handler(&command, &args, &HashMap::new(), handler, budget)
            .await
            .expect("the sidecar connects");
    tokio::time::sleep(ARRIVAL).await;
    let outcome = holon_mcp_client::McpOperationProvider::from_peer_shared(
        peer,
        serde_yaml::from_str("{}").expect("an empty sidecar parses"),
        HashMap::new(),
    )
    .await;
    assert!(
        outcome.is_err(),
        "a sidecar that framed a message past MAX_RESPONSE_BODY_BYTES is still being talked to"
    );
}

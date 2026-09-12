//! Contract: a CONNECTED integration whose sync batches all fail reads
//! `Sync failing` in the `integration_state` row — and reads `Syncing` while
//! its first batch is still in flight.
//!
//! THE ENVIRONMENT GAP this closes: the sync-health fold itself
//! (`SyncHealthSignal`) is pinned in `holon-mcp-client`, but nothing exercised
//! the APP WIRING that subscribes to it — `status_for_sync_health` /
//! `spawn_status_from_sync_health` and the connect-loop branch that calls them
//! (`holon-app/src/mcp_integrations.rs`). Disabling that whole branch left
//! every existing test green, so the feature was structurally unpinned: the
//! row would sit at `Connected` forever while nothing ever landed — exactly the
//! "silently degrades to look fine" failure the error-handling philosophy
//! forbids.
//!
//! HOW the gap is closed: boot the PRODUCTION wiring (`TestEnvironment`) with a
//! real `.state.toml` enabling a connection INTRODUCED by a sidecar file, whose
//! `rest` transport points at a loopback mock this test controls. The mock
//! first HANGS the list call (so the first batch is provably in flight and
//! provably unfinished — the `Syncing` window, observed without a sleep race),
//! then answers HTTP 500 (so every batch fails — the `Sync failing` verdict).
//! A `rest` connection needs no handshake, so the connection CONNECTS while its
//! data never lands: the one shape that separates "the peer is down" from
//! "the peer answers and the rows do not arrive".
//!
//! @pbt kind harness
//! @pbt covers integration-state-sync-failing — a connected integration whose
//! every sync batch fails reads `Sync failing`, and `Syncing` before the first
//! outcome

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use holon::di::DbHandleProvider;
use holon::storage::DbHandle;
use holon_integration_tests::TestEnvironment;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;
use tokio::io::AsyncReadExt;
use tokio::io::AsyncWriteExt;
use tokio::net::TcpListener;

/// Not a bundled name, so the file INTRODUCES the connection and the store
/// still has to switch it on.
const PROVIDER: &str = "failing-rest";
const ENTITY: &str = "failrest_posts";

/// Every wait in this test is for a fact the mock or the mirror publishes, so
/// the bound only has to outlast a cold boot plus the production 2s sync
/// debounce.
const DEADLINE: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Loopback mock: hangs the list call until released, then fails it
// ---------------------------------------------------------------------------

struct MockState {
    /// Hold the response open. While true the sync batch has STARTED and cannot
    /// finish, which is what makes the `Syncing` observation race-free.
    hang: bool,
    requests: usize,
}

struct Mock {
    base_url: String,
    state: Arc<Mutex<MockState>>,
}

impl Mock {
    fn requests(&self) -> usize {
        self.state.lock().unwrap().requests
    }

    /// Let the held call finish — as a 500, so the batch fails.
    fn release(&self) {
        self.state.lock().unwrap().hang = false;
    }
}

async fn start_mock() -> Mock {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind mock");
    let addr = listener.local_addr().expect("mock addr");
    let state = Arc::new(Mutex::new(MockState {
        hang: true,
        requests: 0,
    }));
    let state_bg = state.clone();

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let state_bg = state_bg.clone();
            tokio::spawn(async move {
                let mut buf = Vec::new();
                let mut tmp = [0u8; 1024];
                loop {
                    let n = match socket.read(&mut tmp).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => n,
                    };
                    buf.extend_from_slice(&tmp[..n]);
                    if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                state_bg.lock().unwrap().requests += 1;
                while state_bg.lock().unwrap().hang {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
                let _ = socket
                    .write_all(
                        b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: \
                          close\r\n\r\n",
                    )
                    .await;
                let _ = socket.flush().await;
            });
        }
    });

    Mock {
        base_url: format!("http://{addr}"),
        state,
    }
}

// ---------------------------------------------------------------------------
// The introduced sidecar
// ---------------------------------------------------------------------------

/// The identity column is declared `TEXT` deliberately: any other type is
/// refused at load, and the connection would never connect — which would make
/// this test pass for the wrong reason.
fn sidecar_yaml(base_url: &str) -> String {
    format!(
        r#"
schema_version: {version}
display_name: "Failing Rest"
utcp:
  utcp_version: "1.1.3"
  manual_version: "1.0.0"
  tools:
    - name: list-posts
      tool_call_template:
        call_template_type: http
        url: {base_url}/posts
        http_method: GET
holon:
  tools:
    list-posts:
      result_key: posts
entities:
  {ENTITY}:
    id_column: id
    schema:
      - {{ name: id, sql_type: TEXT, primary_key: true }}
      - {{ name: title, sql_type: TEXT }}
    sync:
      list_tool: list-posts
      extract_path: posts
tools: {{}}
"#,
        version = holon_mcp_client::SIDECAR_SCHEMA_VERSION,
    )
}

// ---------------------------------------------------------------------------
// Mirror reads
// ---------------------------------------------------------------------------

/// The provider's `(enabled, status)` as the Integrations section would read
/// it. Absent row is a distinct answer from a row with no status.
async fn read_row(db: &DbHandle) -> Option<(i64, String)> {
    let rows = db
        .query(
            "SELECT enabled, status FROM integration_state WHERE provider_name = :p",
            HashMap::from([(
                "p".to_string(),
                holon_api::Value::String(PROVIDER.to_string()),
            )]),
        )
        .await
        .expect("query the integration_state mirror");
    rows.first().map(|r| {
        (
            r.get("enabled").and_then(|v| v.as_i64()).expect("enabled"),
            r.get("status")
                .and_then(|v| v.as_string())
                .expect("status")
                .to_string(),
        )
    })
}

/// Poll every 20ms until `cond` holds or `DEADLINE` passes. Real time: the sync
/// loop debounces on the wall clock and the transport does real socket IO.
async fn wait_until<F>(mut cond: F) -> bool
where
    F: AsyncFnMut() -> bool,
{
    let deadline = tokio::time::Instant::now() + DEADLINE;
    while tokio::time::Instant::now() < deadline {
        if cond().await {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cond().await
}

#[test]
fn a_connected_integration_whose_every_sync_fails_reads_sync_failing() {
    let runtime = Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    );
    runtime.clone().block_on(run(runtime.clone()));
}

async fn run(runtime: Arc<tokio::runtime::Runtime>) {
    let mock = start_mock().await;

    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");

    // Install the sidecar and switch it on the way the user does — a file in
    // the config dir's `integrations/`, enabled through the production store,
    // both BEFORE boot.
    let integrations_dir = env.temp_dir.path().join("integrations");
    std::fs::create_dir_all(&integrations_dir).expect("create integrations dir");
    std::fs::write(
        integrations_dir.join(format!("{PROVIDER}.yaml")),
        sidecar_yaml(&mock.base_url),
    )
    .expect("install the sidecar");
    IntegrationConfigStore::load(&integrations_dir)
        .expect("load store")
        .set(
            PROVIDER,
            IntegrationState {
                enabled: true,
                configuration: Configuration::Unconfigured,
            },
        )
        .expect("enable the introduced connection");

    env.start_app(false).await.expect("start_app");

    let db = env
        .injector()
        .expect("start_app must capture the injector")
        .resolve::<dyn DbHandleProvider>()
        .handle();

    // 1. NON-VACUITY. The mock received the list call, so the connection CONNECTED
    //    and its first sync batch is running. A connect failure would have written
    //    `Unavailable` without ever reaching the network, and a refused sidecar
    //    would never have been built at all.
    assert!(
        wait_until(async || mock.requests() >= 1).await,
        "the enabled connection '{PROVIDER}' never called its list tool within {DEADLINE:?} — it \
         did not connect, so nothing below would be testing sync health. Mirror row: {:?}",
        read_row(&db).await
    );

    // 2. The row exists and is enabled, and its status while the first batch is
    //    still in flight (the mock is holding the response open, so no outcome can
    //    have been folded in yet). Read now, asserted below so the headline red is
    //    the `Sync failing` one.
    let in_flight = read_row(&db).await.unwrap_or_else(|| {
        panic!(
            "provider '{PROVIDER}' has no row in the integration_state \
                                   mirror — the enablement store and the mirror have diverged"
        )
    });
    assert_eq!(
        in_flight.0, 1,
        "'{PROVIDER}' is enabled by its state file, so the mirror must say so — got {in_flight:?}"
    );

    // 3. Let the held call fail. Every batch now fails and none has ever succeeded,
    //    so the fold reaches `Failing`.
    mock.release();

    // 4. THE RED. The app must follow the sync-health signal and write the verdict
    //    the batches justify.
    let reached = wait_until(async || {
        read_row(&db).await.map(|(_, s)| s) == Some("Sync failing".to_string())
    })
    .await;
    assert!(
        reached,
        "a CONNECTED integration whose every sync batch failed must read exactly `Sync failing` \
         in integration_state, but '{PROVIDER}' reads {:?} after {DEADLINE:?}. The connection \
         answered (its list tool was called {} times) and every call failed with HTTP 500, so the \
         row is claiming a health its data never had.",
        read_row(&db).await,
        mock.requests()
    );

    // 5. The watcher that wrote that verdict holds a `DbHandle` and must not
    //    outlive the store. Asserted HERE because this is the only test that boots
    //    a connection that actually connects, so it is the only place an orphaned
    //    sync-status task is observable at all — `session_shutdown_stops_watchers`
    //    boots no integration and would pass with this family unregistered.
    let shutdown = env
        .injector()
        .expect("start_app must capture the injector")
        .resolve::<holon_api::lifecycle::SessionShutdown>();
    let registered = shutdown.registered();
    assert!(
        registered.iter().any(|n| n == "integration-sync-status"),
        "the per-integration sync-status watcher is not registered with the session shutdown, so \
         nothing stops it before the store closes and it would keep writing through a dead \
         actor. Registered: {registered:?}"
    );

    // 5. And before that first outcome the row said `Syncing` — the word that keeps
    //    "waiting for data" distinguishable from both `Pending` (no status ever
    //    landed) and `Connected` (rows are landing).
    assert_eq!(
        in_flight.1, "Syncing",
        "while the first sync batch was in flight (the mock was holding the response open), \
         '{PROVIDER}' must read `Syncing` — got {:?}",
        in_flight.1
    );
}

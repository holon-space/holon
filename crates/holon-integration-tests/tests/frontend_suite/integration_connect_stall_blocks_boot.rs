//! Contract: an enabled integration whose connect has not completed does not
//! hold the session hostage — `FrontendSession` still resolves, so the window,
//! the embedded MCP server and the post-scan sync gate all come up — and an
//! integration that connects after the session resolved comes alive without a
//! restart.
//!
//! Dogfood escape
//! `2026-10-06-release-boot-stalls-behind-integration-keychain-prompt`: the
//! live release boot waited on a keychain prompt inside one integration's
//! connect, and the `FrontendSession` factory never returned.
//!
//! @pbt kind harness
//! @pbt covers integration-connect-stall-boot — a connect that never completes
//! does not block FrontendSession resolution
//! @pbt covers integration-connect-timing — an integration that connects after
//! the session resolved reaches the dispatcher, the profiles and MCP discovery

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use std::time::Instant;

use holon::di::DbHandleProvider;
use holon_core::OperationProvider;
use holon_integration_tests::TestEnvironment;
use holon_integration_tests::fake_mcp_module::FakeMcpPeer;
use holon_integration_tests::fake_mcp_module::PROVIDER_NAME;
use holon_integration_tests::fake_mcp_module::WRITE_OP;
use holon_integration_tests::fake_mcp_module::WRITTEN_ENTITY;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;
use holon_pbt_core::capabilities::IntegrationConnectTiming;
use tokio::net::TcpListener;

const STALLED_PROVIDER: &str = "stalled-peer";
const PAGINATING_PROVIDER: &str = "paginating-peer";

/// A cold boot of the test wiring finishes in a few seconds; a boot that waits
/// on a connect does not finish at all.
const BOOT_BOUND: Duration = Duration::from_secs(30);

/// How long a released peer may take to connect and register its operations.
const CONNECT_BOUND: Duration = Duration::from_secs(20);

/// Accepts every connection and keeps it open without ever writing a byte.
async fn start_silent_peer() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
    let addr = listener.local_addr().expect("peer addr");
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((socket, _)) = listener.accept().await {
            held.push(socket);
        }
    });
    format!("http://{addr}/mcp")
}

fn sidecar_yaml(uri: &str) -> String {
    format!(
        r#"
schema_version: {version}
display_name: "Stalled Peer"
transport:
  http:
    uri: "{uri}"
entities: {{}}
tools: {{}}
"#,
        version = holon_mcp_client::SIDECAR_SCHEMA_VERSION,
    )
}

fn runtime() -> Arc<tokio::runtime::Runtime> {
    Arc::new(
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(4)
            .enable_all()
            .build()
            .unwrap(),
    )
}

#[test]
fn an_integration_whose_connect_never_completes_does_not_block_the_session() {
    let runtime = runtime();
    runtime.clone().block_on(never_answers(runtime.clone()));
}

async fn never_answers(runtime: Arc<tokio::runtime::Runtime>) {
    let uri = start_silent_peer().await;
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");

    let integrations_dir = env.temp_dir.path().join("integrations");
    std::fs::create_dir_all(&integrations_dir).expect("create integrations dir");
    std::fs::write(
        integrations_dir.join(format!("{STALLED_PROVIDER}.yaml")),
        sidecar_yaml(&uri),
    )
    .expect("install the sidecar");
    IntegrationConfigStore::load(&integrations_dir)
        .expect("load store")
        .set(
            STALLED_PROVIDER,
            IntegrationState {
                enabled: true,
                configuration: Configuration::Unconfigured,
            },
        )
        .expect("enable the introduced connection");

    let started = tokio::time::timeout(BOOT_BOUND, env.start_app(false)).await;
    assert!(
        started.is_ok(),
        "FrontendSession did not resolve within {BOOT_BOUND:?}: the integration \
         '{STALLED_PROVIDER}' connects to a peer that never answers, and the session waits on \
         that connect — so every consumer of the session (the window, the embedded MCP server, \
         the post_ready sync gate) waits on one integration forever"
    );
    started
        .unwrap()
        .expect("start_app failed after resolving the session");
}

#[test]
fn an_integration_that_connects_after_the_session_resolved_comes_alive() {
    let runtime = runtime();
    runtime.clone().block_on(answers_late(runtime.clone()));
}

async fn answers_late(runtime: Arc<tokio::runtime::Runtime>) {
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");
    let peer = FakeMcpPeer::start(IntegrationConnectTiming::AfterSessionResolve).await;
    peer.install(env.temp_dir.path());

    tokio::time::timeout(BOOT_BOUND, env.start_app(false))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "FrontendSession did not resolve within {BOOT_BOUND:?} while the '{PROVIDER_NAME}' \
                 peer was held: the session waits on the integration's connect"
            )
        })
        .expect("start_app");

    let status = poll_status(&env, |s| s == "Connecting", Duration::from_secs(5)).await;
    assert_eq!(
        status.as_deref(),
        Some("Connecting"),
        "a held peer's integration must read Connecting once the session is up"
    );
    let held = surfaces(&env).await;
    assert!(
        !held.dispatcher.contains(WRITE_OP),
        "the held integration's write is already in the dispatcher catalog: {held:?}"
    );
    let refusal = env
        .engine()
        .execute_operation(
            &holon_api::EntityName::new(WRITTEN_ENTITY),
            WRITE_OP,
            HashMap::new(),
            holon_api::OpOrigin::User,
        )
        .await
        .expect_err("a write to a not-yet-connected integration must be refused");
    let refusal = format!("{refusal:#}");
    assert!(
        refusal.contains(PROVIDER_NAME) && refusal.contains("Connecting"),
        "the refusal must name the integration and its state: {refusal}"
    );

    peer.release();

    let deadline = Instant::now() + CONNECT_BOUND;
    let connected = loop {
        let now = surfaces(&env).await;
        if now.dispatcher.contains(WRITE_OP)
            && now.profile.contains(WRITE_OP)
            && now.mcp.contains(WRITE_OP)
        {
            break now;
        }
        assert!(
            Instant::now() < deadline,
            "{CONNECT_BOUND:?} after the release, '{WRITTEN_ENTITY}.{WRITE_OP}' has not reached \
             every surface: {now:?}"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    };
    assert_eq!(
        connected.dispatcher, connected.profile,
        "the profile resolver must offer what the dispatcher accepts"
    );
    let status = poll_status(
        &env,
        |s| s == "Syncing" || s == "Connected",
        Duration::from_secs(5),
    )
    .await;
    assert!(
        matches!(status.as_deref(), Some("Syncing" | "Connected")),
        "a connected integration must read Syncing or Connected, got {status:?}"
    );

    let unknown = env
        .engine()
        .execute_operation(
            &holon_api::EntityName::new(WRITTEN_ENTITY),
            "no_such_op",
            HashMap::new(),
            holon_api::OpOrigin::User,
        )
        .await
        .expect_err("an operation no provider offers must fail");
    let unknown = format!("{unknown:#}");
    assert!(
        unknown.contains("No provider registered for entity") && !unknown.contains(PROVIDER_NAME),
        "an unknown operation on a connected integration is a wiring error, not the \
         integration's state: {unknown}"
    );
}

#[derive(Debug)]
struct Surfaces {
    dispatcher: BTreeSet<String>,
    profile: BTreeSet<String>,
    mcp: BTreeSet<String>,
}

async fn surfaces(env: &TestEnvironment) -> Surfaces {
    let engine = env.engine();
    let entity = holon_api::EntityName::new(WRITTEN_ENTITY);
    let dispatcher = engine
        .get_dispatcher()
        .operations()
        .into_iter()
        .filter(|op| op.entity_name == entity)
        .map(|op| op.name)
        .collect();
    let profile = engine
        .profile_resolver()
        .operations_for(WRITTEN_ENTITY)
        .into_iter()
        .map(|op| op.name)
        .collect();
    Surfaces {
        dispatcher,
        profile,
        mcp: mcp_list_operations(env).await,
    }
}

/// The MCP `list_operations` tool over an in-process transport.
async fn mcp_list_operations(env: &TestEnvironment) -> BTreeSet<String> {
    use rmcp::ServiceExt;

    let server = holon_mcp::server::HolonMcpServer::with_type_registry(
        Some(env.engine().clone()),
        None,
        env.debug_services()
            .expect("start_app populates the debug services")
            .clone(),
        None,
    );
    let (server_transport, client_transport) = tokio::io::duplex(64 * 1024);
    let (server_running, client_running) = tokio::try_join!(
        async {
            server
                .serve(server_transport)
                .await
                .map_err(anyhow::Error::from)
        },
        async { ().serve(client_transport).await.map_err(anyhow::Error::from) },
    )
    .expect("in-process MCP handshake");
    let result = client_running
        .peer()
        .call_tool(rmcp::model::CallToolRequestParam {
            name: "list_operations".into(),
            arguments: serde_json::json!({ "entity_name": WRITTEN_ENTITY })
                .as_object()
                .cloned(),
        })
        .await
        .expect("list_operations over MCP");
    client_running
        .cancel()
        .await
        .expect("MCP client shuts down");
    server_running
        .cancel()
        .await
        .expect("MCP server shuts down");
    let text = result
        .content
        .iter()
        .find_map(|c| c.as_text().map(|t| t.text.clone()))
        .expect("list_operations answers with text");
    let ops: Vec<serde_json::Value> =
        serde_json::from_str(&text).expect("list_operations answers a JSON array");
    ops.iter()
        .map(|op| {
            op["name"]
                .as_str()
                .expect("every listed operation has a name")
                .to_string()
        })
        .collect()
}

/// The integration's `integration_state` status once `accept` holds, or the
/// last value seen when `within` runs out.
async fn poll_status(
    env: &TestEnvironment,
    accept: impl Fn(&str) -> bool,
    within: Duration,
) -> Option<String> {
    poll_provider_status(env, PROVIDER_NAME, accept, within).await
}

async fn poll_provider_status(
    env: &TestEnvironment,
    provider: &str,
    accept: impl Fn(&str) -> bool,
    within: Duration,
) -> Option<String> {
    let db = env
        .injector()
        .expect("start_app captures the injector")
        .resolve::<dyn DbHandleProvider>()
        .handle();
    let deadline = Instant::now() + within;
    loop {
        let rows = db
            .query(
                "SELECT status FROM integration_state WHERE provider_name = :p",
                HashMap::from([(
                    "p".to_string(),
                    holon_api::Value::String(provider.to_string()),
                )]),
            )
            .await
            .expect("query the integration_state mirror");
        let status = rows.first().map(|r| {
            r.get("status")
                .and_then(|v| v.as_string())
                .expect("status column")
                .to_string()
        });
        if status.as_deref().is_some_and(&accept) || Instant::now() >= deadline {
            return status;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// Answers `initialize`, then answers every `tools/list` with one tool and
/// always one more cursor. The connect's tool enumeration is a loop the PEER
/// drives, and every per-request deadline stays satisfied while it runs.
async fn start_endless_pagination_peer() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind peer");
    let addr = listener.local_addr().expect("peer addr");
    tokio::spawn(async move {
        while let Ok((sock, _)) = listener.accept().await {
            tokio::spawn(paginate_forever(sock));
        }
    });
    format!("http://{addr}/mcp")
}

async fn paginate_forever(sock: tokio::net::TcpStream) {
    use tokio::io::AsyncBufReadExt as _;
    use tokio::io::AsyncReadExt as _;
    use tokio::io::AsyncWriteExt as _;
    let (read, mut write) = sock.into_split();
    let mut head = tokio::io::BufReader::new(read);
    loop {
        let mut length = 0usize;
        let mut is_post = false;
        loop {
            let mut line = String::new();
            if head.read_line(&mut line).await.unwrap_or(0) == 0 {
                return;
            }
            if line == "\r\n" {
                break;
            }
            is_post |= line.starts_with("POST ");
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().expect("a numeric Content-Length");
            }
        }
        if !is_post {
            // The notification GET stream: refused, to keep this peer to the
            // enumeration leg under test.
            let _ = write
                .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")
                .await;
            continue;
        }
        let mut body = vec![0u8; length];
        if head.read_exact(&mut body).await.is_err() {
            return;
        }
        let msg: serde_json::Value = serde_json::from_slice(&body).expect("a JSON-RPC body");
        if msg["id"].is_null() {
            let _ = write
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                .await;
            continue;
        }
        let result = match msg["method"].as_str().unwrap_or_default() {
            "initialize" => serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "paginating", "version": "0"},
            }),
            "tools/list" => serde_json::json!({
                "tools": [{"name": "one", "description": "a tool",
                           "inputSchema": {"type": "object"}}],
                "nextCursor": "there is always one more page",
            }),
            _ => serde_json::json!({}),
        };
        let reply = serde_json::json!({"jsonrpc": "2.0", "id": msg["id"], "result": result});
        let reply = serde_json::to_vec(&reply).expect("serialize the reply");
        let head_bytes = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: \
             paginating\r\nContent-Length: {}\r\n\r\n",
            reply.len()
        );
        if write.write_all(head_bytes.as_bytes()).await.is_err()
            || write.write_all(&reply).await.is_err()
        {
            return;
        }
    }
}

#[test]
fn a_peer_that_paginates_forever_degrades_only_its_own_integration() {
    let runtime = runtime();
    runtime.clone().block_on(paginates_forever(runtime.clone()));
}

/// Boot must come up AND the hostile integration must reach a terminal state.
/// A connect that never returns satisfies the first half on its own (the
/// supervisor spawns it), so the second half is what the peer budget adds: the
/// enumeration ends, the integration reads `Unavailable`, and the user is told
/// instead of looking at a blank page for a connection that says `Connecting`
/// forever.
async fn paginates_forever(runtime: Arc<tokio::runtime::Runtime>) {
    let uri = start_endless_pagination_peer().await;
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");

    let integrations_dir = env.temp_dir.path().join("integrations");
    std::fs::create_dir_all(&integrations_dir).expect("create integrations dir");
    std::fs::write(
        integrations_dir.join(format!("{PAGINATING_PROVIDER}.yaml")),
        sidecar_yaml(&uri),
    )
    .expect("install the sidecar");
    IntegrationConfigStore::load(&integrations_dir)
        .expect("load store")
        .set(
            PAGINATING_PROVIDER,
            IntegrationState {
                enabled: true,
                configuration: Configuration::Unconfigured,
            },
        )
        .expect("enable the introduced connection");

    tokio::time::timeout(BOOT_BOUND, env.start_app(false))
        .await
        .unwrap_or_else(|_| {
            panic!(
                "FrontendSession did not resolve within {BOOT_BOUND:?} while \
                 '{PAGINATING_PROVIDER}' enumerated a peer's endless tool list"
            )
        })
        .expect("start_app failed after resolving the session");

    let status = poll_provider_status(
        &env,
        PAGINATING_PROVIDER,
        |s| s == "Unavailable",
        CONNECT_BOUND,
    )
    .await;
    assert_eq!(
        status.as_deref(),
        Some("Unavailable"),
        "{CONNECT_BOUND:?} after boot, '{PAGINATING_PROVIDER}' still does not read Unavailable \
         (got {status:?}) — a peer that answers every page with one more cursor keeps the connect \
         enumerating, so the integration never reaches a state the user can act on and the \
         process grows for as long as the peer wants"
    );
}

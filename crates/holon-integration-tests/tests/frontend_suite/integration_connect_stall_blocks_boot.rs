//! Contract: an enabled integration whose connect never completes does not
//! hold the session hostage — `FrontendSession` still resolves, so the window,
//! the embedded MCP server and the post-scan sync gate all come up.
//!
//! Dogfood escape
//! `2026-10-06-release-boot-stalls-behind-integration-keychain-prompt`.
//! The registry connects every integration serially INSIDE the
//! `BackendEngine` resolve (the dispatcher collects their operation providers),
//! and nothing bounds a connect. On the live release boot the last integration
//! blocked on a keychain prompt, and the `FrontendSession` factory never
//! returned: no MCP listener, and `post_ready` never opened the sync gate.
//!
//! The loopback peer here accepts the connection and never answers — the
//! same shape for the boot as a credential lookup that never returns.
//!
//! @pbt kind harness
//! @pbt covers integration-connect-stall-boot — a connect that never completes
//! does not block FrontendSession resolution

use std::sync::Arc;
use std::time::Duration;

use holon_integration_tests::TestEnvironment;
use holon_mcp_client::IntegrationConfigStore;
use holon_mcp_client::integration_state::Configuration;
use holon_mcp_client::integration_state::IntegrationState;
use tokio::net::TcpListener;

const PROVIDER: &str = "stalled-peer";

/// A cold boot of the test wiring finishes in a few seconds; the stall is
/// unbounded, so any finite bound separates the two.
const BOOT_BOUND: Duration = Duration::from_secs(60);

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

#[test]
#[ignore = "red until D109 Inc 3: the boot still awaits every integration connect"]
fn an_integration_whose_connect_never_completes_does_not_block_the_session() {
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
    let uri = start_silent_peer().await;
    let env = TestEnvironment::new(runtime).expect("new TestEnvironment");

    let integrations_dir = env.temp_dir.path().join("integrations");
    std::fs::create_dir_all(&integrations_dir).expect("create integrations dir");
    std::fs::write(
        integrations_dir.join(format!("{PROVIDER}.yaml")),
        sidecar_yaml(&uri),
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

    let started = tokio::time::timeout(BOOT_BOUND, env.start_app(false)).await;
    assert!(
        started.is_ok(),
        "FrontendSession did not resolve within {BOOT_BOUND:?}: the integration '{PROVIDER}' \
         connects to a peer that never answers, and the registry connect loop runs inside the \
         BackendEngine resolve with no bound — so every consumer of the session (the window, the \
         embedded MCP server, the post_ready sync gate) waits on one integration forever"
    );
    started
        .unwrap()
        .expect("start_app failed after resolving the session");
}

//! `resources/subscribe` is a request an untrusted peer answers, so it carries
//! the same deadline every other MCP request carries.
//!
//! Two callers wait on `subscribe_all`: the connect path
//! (`finish_integration`) and the FDW cache prime (`MatviewManager`), so a peer
//! that never answers it stalls both.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;

use async_trait::async_trait;
use holon_api::StreamPosition;
use holon_core::SyncTokenStore;
use holon_mcp_client::McpSidecar;
use holon_mcp_client::McpSyncEngine;
use holon_mcp_client::VtableSubscription;
use holon_mcp_client::mcp_call_surface::McpCallSurface;
use rmcp::ServiceExt as _;
use tokio::io::AsyncBufReadExt as _;
use tokio::io::AsyncWriteExt as _;

/// The deadline the crate puts on every MCP request. Private to the crate, so
/// the test states it and asserts the wait is near it rather than unbounded.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

struct NoopTokenStore;

#[async_trait]
impl SyncTokenStore for NoopTokenStore {
    async fn load_token(&self, _: &str) -> holon_core::Result<Option<StreamPosition>> {
        Ok(None)
    }
    async fn save_token(&self, _: &str, _: StreamPosition) -> holon_core::Result<()> {
        Ok(())
    }
    async fn clear_all_tokens(&self) -> holon_core::Result<()> {
        Ok(())
    }
}

/// A peer that answers `initialize` and nothing else, recording every method
/// it was sent.
fn silent_peer(server_io: tokio::io::DuplexStream) -> Arc<Mutex<Vec<String>>> {
    let methods = Arc::new(Mutex::new(Vec::new()));
    let seen = methods.clone();
    let (read, mut write) = tokio::io::split(server_io);
    tokio::spawn(async move {
        let mut lines = tokio::io::BufReader::new(read).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            let msg: serde_json::Value = serde_json::from_str(&line).expect("JSON-RPC line");
            let method = msg["method"].as_str().unwrap_or_default().to_string();
            seen.lock().expect("lock").push(method.clone());
            if method == "initialize" {
                let reply = serde_json::json!({"jsonrpc": "2.0", "id": msg["id"], "result": {
                    "protocolVersion": "2025-03-26",
                    "capabilities": {"tools": {}, "resources": {"subscribe": true}},
                    "serverInfo": {"name": "silent", "version": "0"}}});
                write
                    .write_all(format!("{reply}\n").as_bytes())
                    .await
                    .expect("write");
            }
        }
    });
    methods
}

#[tokio::test(start_paused = true)]
async fn a_peer_that_never_answers_subscribe_is_cut_at_the_request_timeout() {
    let (client_io, server_io) = tokio::io::duplex(1 << 16);
    let methods = silent_peer(server_io);
    let client = ().serve(client_io).await.expect("handshake");
    let peer = client.peer().clone();
    let surface: Arc<dyn McpCallSurface> = Arc::new(peer.clone());

    let engine = McpSyncEngine::new(
        surface,
        Some(peer),
        HashMap::new(),
        HashMap::new(),
        Arc::new(NoopTokenStore),
        "silent".to_string(),
        serde_yaml::from_str::<McpSidecar>("{}").expect("an empty sidecar parses"),
        vec![VtableSubscription {
            uri_template: "silent://rows".to_string(),
            fdw_table: "silent_rows_fdw".to_string(),
            param_columns: Vec::new(),
        }],
        None,
    );

    let started = tokio::time::Instant::now();
    let outcome = tokio::time::timeout(REQUEST_TIMEOUT * 2, engine.subscribe_all())
        .await
        .unwrap_or_else(|_| {
            panic!(
                "subscribe_all was still waiting after {:?} — a peer that never answers \
                 `resources/subscribe` stalls the connect and the FDW prime",
                REQUEST_TIMEOUT * 2
            )
        });
    let err = format!(
        "{:#}",
        outcome.expect_err("a subscribe the peer never answers is no subscription")
    );
    assert!(
        err.contains("REQUEST_TIMEOUT") && err.contains("silent://rows"),
        "the error must name the deadline and the resource; got: {err}"
    );
    assert!(
        started.elapsed() <= REQUEST_TIMEOUT + Duration::from_secs(1),
        "the wait must end at the deadline; it took {:?}",
        started.elapsed()
    );

    tokio::time::sleep(Duration::from_secs(1)).await;
    let seen = methods.lock().expect("lock").clone();
    assert!(
        seen.iter().any(|m| m == "notifications/cancelled"),
        "the peer must be told to stop the subscribe it never answered; it saw: {seen:?}"
    );
}

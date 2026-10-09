//! A peer's resource-template list is a second enumeration the connect runs,
//! and it was the one failure the connect swallowed: a `warn!` plus an empty
//! list, so a peer whose templates could not be read looked exactly like a peer
//! that publishes none. The entities those templates would have discovered then
//! quietly do not exist.
//!
//! The connect does NOT fail over it — the tool list is the dispatch surface
//! and it came through — but the integration is degraded and must say so, with
//! the reason, so the app can disclose it.
//!
//! Driven through the production `build_mcp_integration` over real HTTP.

use std::io::Write as _;
use std::sync::Arc;

use holon::di::DbHandleCacheFactory;
use holon::storage::DbHandle;
use holon_api::StreamPosition;
use holon_core::SyncTokenStore;
use holon_mcp_client::AuthMode;
use holon_mcp_client::McpConnectionResult;
use holon_mcp_client::McpIntegrationConfig;
use holon_mcp_client::McpTransport;
use holon_mcp_client::PendingOAuthFlows;
use holon_mcp_client::SyncGate;
use holon_mcp_client::build_mcp_integration;
use holon_turso::turso::TursoBackend;

struct NoopTokenStore;

#[async_trait::async_trait]
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

/// A peer that answers `tools/list` in ONE complete page and then paginates
/// `resources/templates/list` forever. Only the second enumeration is hostile,
/// so a connect that reports success is reporting a half-truth.
fn templates_paginate_forever() -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind the peer");
    let uri = format!("http://{}/mcp", listener.local_addr().expect("peer addr"));
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            std::thread::spawn(move || serve(stream));
        }
    });
    uri
}

fn serve(mut stream: std::net::TcpStream) {
    use std::io::BufRead as _;
    let peer = stream.try_clone().expect("clone the socket");
    let mut head = std::io::BufReader::new(peer);
    loop {
        let mut length = 0usize;
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
                length = v.trim().parse().expect("a numeric Content-Length");
            }
        }
        if !is_post {
            stream
                .write_all(b"HTTP/1.1 405 Method Not Allowed\r\nContent-Length: 0\r\n\r\n")
                .expect("refuse the GET stream");
            continue;
        }
        let mut body = vec![0u8; length];
        std::io::Read::read_exact(&mut head, &mut body).expect("the announced body arrives");
        let msg: serde_json::Value = serde_json::from_slice(&body).expect("a JSON-RPC body");
        if msg["id"].is_null() {
            stream
                .write_all(b"HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")
                .expect("ack a notification");
            continue;
        }
        let result = match msg["method"].as_str().unwrap_or_default() {
            "initialize" => serde_json::json!({
                "protocolVersion": "2025-03-26",
                "capabilities": {"tools": {}, "resources": {}},
                "serverInfo": {"name": "half-hostile", "version": "0"},
            }),
            // Complete, and the only page.
            "tools/list" => serde_json::json!({"tools": []}),
            "resources/templates/list" => serde_json::json!({
                "resourceTemplates": [],
                "nextCursor": "there is always one more page",
            }),
            _ => serde_json::json!({}),
        };
        let reply = serde_json::json!({"jsonrpc": "2.0", "id": msg["id"], "result": result});
        let reply = serde_json::to_vec(&reply).expect("serialize the reply");
        stream
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: \
                     half\r\nContent-Length: {}\r\n\r\n",
                    reply.len()
                )
                .as_bytes(),
            )
            .expect("write the reply head");
        stream.write_all(&reply).expect("write the reply body");
    }
}

async fn setup_db() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    holon_turso::schema_module::SchemaModule::ensure_schema(
        &holon_turso::schema_modules::CoreSchemaModule,
        &handle,
    )
    .await
    .expect("core schema");
    handle
}

#[tokio::test(flavor = "multi_thread")]
async fn a_peer_whose_template_list_cannot_be_read_connects_but_says_discovery_is_incomplete() {
    let db = setup_db().await;
    let config = McpIntegrationConfig {
        provider_name: "half".to_string(),
        transport: McpTransport::Http {
            uri: templates_paginate_forever(),
        },
        sidecar_yaml: format!(
            "schema_version: {}\ndisplay_name: \"Half\"\nentities: {{}}\ntools: {{}}\n",
            holon_mcp_client::SIDECAR_SCHEMA_VERSION
        ),
        auth_mode: AuthMode::None,
    };
    let built = tokio::time::timeout(
        std::time::Duration::from_secs(60),
        build_mcp_integration(
            config,
            db.clone(),
            Arc::new(DbHandleCacheFactory::new(db.clone())),
            Arc::new(NoopTokenStore) as Arc<dyn SyncTokenStore>,
            &PendingOAuthFlows::new(),
            SyncGate::opened(),
        ),
    )
    .await
    .expect("the connect was still enumerating resource templates after 60s")
    .expect("the tool list came through, so the integration must still connect");

    let McpConnectionResult::Connected(integration) = built else {
        panic!("a peer whose TOOL list is fine must still connect");
    };
    let reason = integration.discovery_incomplete.as_deref().expect(
        "the integration came up without the entities its resource templates would have \
             discovered, and reported nothing — an unreadable template list is indistinguishable \
             from a peer that publishes none",
    );
    assert!(
        reason.contains("MAX_LIST_PAGES") && reason.contains("list_resource_templates"),
        "the reason must name the enumeration and the bound it hit; got: {reason}"
    );
}

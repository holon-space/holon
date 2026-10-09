//! Thin trait abstraction over the rmcp `Peer<RoleClient>` methods used by the
//! FDW. Exists so tests can drive the MCP fan-out logic with a scripted peer
//! instead of standing up a live rmcp transport.
//!
//! Only the two methods actually called by `McpCursor` are abstracted —
//! `call_tool` and `read_resource`. Everything else on `Peer` is unused by the
//! FDW and intentionally not part of this surface.

use async_trait::async_trait;
use rmcp::RoleClient;
use rmcp::model::CallToolRequest;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::ClientRequest;
use rmcp::model::ReadResourceRequest;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ReadResourceResult;
use rmcp::model::ServerResult;
use rmcp::service::Peer;
use rmcp::service::ServiceError;

/// One call a connection DECLARES for itself, as an operation descriptor needs
/// it. Independent of how the call is reached: a peer answers
/// `list_all_tools` with the same three facts a manual publishes under
/// `utcp.tools`.
pub struct DeclaredCall {
    /// The name the connection answers to, in the connection's own spelling.
    pub name: String,
    pub description: Option<String>,
    /// The published input schema. `None` means the connection publishes NONE,
    /// which is not the same as publishing an empty one: an empty schema
    /// declares a call that takes no arguments.
    pub input_schema: Option<serde_json::Map<String, serde_json::Value>>,
}

#[async_trait]
pub trait McpCallSurface: Send + Sync + std::fmt::Debug {
    async fn call_tool(&self, params: CallToolRequestParam)
    -> Result<CallToolResult, ServiceError>;

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError>;

    /// The rows one call's response maps to, per the connection's declared
    /// `response` filter.
    ///
    /// Defaulted to a loud refusal because only a `utcp:` connection carries
    /// mappings: an MCP peer's tools are typed by the server, so a caller that
    /// asked one for rows has confused two kinds of connection, and answering
    /// with an empty set would delete everything that call owns under
    /// replace-scope semantics.
    fn map_response(
        &self,
        call_name: &str,
        _: &serde_json::Value,
    ) -> anyhow::Result<Vec<holon_core::file_format::TypedRowSet>> {
        anyhow::bail!(
            "call '{call_name}' reaches an MCP peer, which declares no `response` mapping; only a \
             `utcp:` connection maps a response into rows"
        )
    }

    /// The call arguments a row stream maps to — the write leg of
    /// [`Self::map_response`], refused on the same grounds.
    fn map_request(
        &self,
        call_name: &str,
        _: &serde_json::Value,
    ) -> anyhow::Result<serde_json::Map<String, serde_json::Value>> {
        anyhow::bail!(
            "call '{call_name}' reaches an MCP peer, which declares no `request` mapping; only a \
             `utcp:` connection maps rows into a call"
        )
    }
}

/// Extract the structured JSON response object from an MCP tool result.
///
/// Prefers the spec'd `structured_content` field. Some servers (e.g. the
/// official Todoist MCP at `ai.todoist.net/mcp`) return BOTH a JSON text block
/// AND a trailing human-readable prose summary as a second text block; naively
/// concatenating all text blocks and parsing the result as JSON fails on the
/// trailing prose. So when `structured_content` is absent we fall back to the
/// first text content block that parses as valid JSON, rather than the join.
pub fn extract_tool_response(result: &CallToolResult) -> anyhow::Result<serde_json::Value> {
    if let Some(structured) = &result.structured_content {
        return Ok(structured.clone());
    }

    let mut last_err: Option<serde_json::Error> = None;
    for content in &result.content {
        let Some(text) = content.as_text() else {
            continue;
        };
        match serde_json::from_str::<serde_json::Value>(&text.text) {
            Ok(value) => return Ok(value),
            Err(e) => last_err = Some(e),
        }
    }

    match last_err {
        Some(e) => Err(anyhow::anyhow!(
            "No tool content block parsed as JSON (and no structured_content): {e}"
        )),
        None => Err(anyhow::anyhow!(
            "Tool result had no text content and no structured_content"
        )),
    }
}

#[async_trait]
impl McpCallSurface for Peer<RoleClient> {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        let what = format!("call_tool '{}'", params.name);
        let request = ClientRequest::CallToolRequest(CallToolRequest {
            method: Default::default(),
            params,
            extensions: Default::default(),
        });
        match crate::mcp_request::request(self, &what, request).await? {
            ServerResult::CallToolResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        let what = format!("read_resource '{}'", params.uri);
        let request = ClientRequest::ReadResourceRequest(ReadResourceRequest {
            method: Default::default(),
            params,
            extensions: Default::default(),
        });
        match crate::mcp_request::request(self, &what, request).await? {
            ServerResult::ReadResourceResult(result) => Ok(result),
            _ => Err(ServiceError::UnexpectedResponse),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex;

    use rmcp::ServiceExt as _;
    use tokio::io::AsyncBufReadExt as _;
    use tokio::io::AsyncWriteExt as _;

    use super::*;
    use crate::secure_client::REQUEST_TIMEOUT;

    /// A peer that answers `initialize`, then answers every request with one
    /// space a second and never a complete message.
    fn trickling_peer(server_io: tokio::io::DuplexStream) -> Arc<Mutex<Vec<String>>> {
        let methods = Arc::new(Mutex::new(Vec::new()));
        let seen = methods.clone();
        let (read, write) = tokio::io::split(server_io);
        let write = Arc::new(tokio::sync::Mutex::new(write));
        tokio::spawn(async move {
            let mut lines = tokio::io::BufReader::new(read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: serde_json::Value = serde_json::from_str(&line).expect("JSON-RPC line");
                let method = msg["method"].as_str().unwrap_or_default().to_string();
                seen.lock().expect("lock").push(method.clone());
                if method == "initialize" {
                    let reply = serde_json::json!({"jsonrpc": "2.0", "id": msg["id"], "result": {
                        "protocolVersion": "2025-03-26",
                        "capabilities": {"tools": {}, "resources": {}},
                        "serverInfo": {"name": "trickle", "version": "0"}}});
                    let mut w = write.lock().await;
                    w.write_all(format!("{reply}\n").as_bytes())
                        .await
                        .expect("write");
                } else if msg.get("id").is_some() {
                    let write = write.clone();
                    tokio::spawn(async move {
                        loop {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            if write.lock().await.write_all(b" ").await.is_err() {
                                return;
                            }
                        }
                    });
                }
            }
        });
        methods
    }

    #[tokio::test(start_paused = true)]
    async fn a_peer_that_trickles_is_cut_at_the_request_timeout_naming_the_call() {
        let (client_io, server_io) = tokio::io::duplex(1 << 16);
        let methods = trickling_peer(server_io);
        let client = ().serve(client_io).await.expect("handshake");
        let peer = client.peer().clone();

        let started = tokio::time::Instant::now();
        let call = tokio::time::timeout(
            REQUEST_TIMEOUT * 2,
            McpCallSurface::call_tool(
                &peer,
                CallToolRequestParam {
                    name: "slow-tool".into(),
                    arguments: None,
                },
            ),
        )
        .await
        .unwrap_or_else(|_| panic!("call_tool was still open after {:?}", REQUEST_TIMEOUT * 2));
        let err = call
            .expect_err("a trickled answer is no answer")
            .to_string();
        assert!(
            err.contains("REQUEST_TIMEOUT") && err.contains("'slow-tool'"),
            "the error must name the deadline and the tool; got: {err}"
        );
        assert!(started.elapsed() <= REQUEST_TIMEOUT + std::time::Duration::from_secs(1));

        let read = tokio::time::timeout(
            REQUEST_TIMEOUT * 2,
            McpCallSurface::read_resource(
                &peer,
                ReadResourceRequestParam {
                    uri: "slow://resource".into(),
                },
            ),
        )
        .await
        .unwrap_or_else(|_| {
            panic!(
                "read_resource was still open after {:?}",
                REQUEST_TIMEOUT * 2
            )
        });
        let err = read
            .expect_err("a trickled answer is no answer")
            .to_string();
        assert!(
            err.contains("REQUEST_TIMEOUT") && err.contains("'slow://resource'"),
            "the error must name the deadline and the resource; got: {err}"
        );

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        let cancelled = methods
            .lock()
            .expect("lock")
            .iter()
            .filter(|m| *m == "notifications/cancelled")
            .count();
        assert_eq!(cancelled, 2, "the peer is told to stop both requests");
    }
}

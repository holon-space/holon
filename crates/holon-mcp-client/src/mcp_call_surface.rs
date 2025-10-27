//! Thin trait abstraction over the rmcp `Peer<RoleClient>` methods used by the
//! FDW. Exists so tests can drive the MCP fan-out logic with a scripted peer
//! instead of standing up a live rmcp transport.
//!
//! Only the two methods actually called by `McpCursor` are abstracted —
//! `call_tool` and `read_resource`. Everything else on `Peer` is unused by the
//! FDW and intentionally not part of this surface.

use async_trait::async_trait;
use rmcp::RoleClient;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ReadResourceResult;
use rmcp::service::Peer;
use rmcp::service::ServiceError;

#[async_trait]
pub trait McpCallSurface: Send + Sync + std::fmt::Debug {
    async fn call_tool(&self, params: CallToolRequestParam)
    -> Result<CallToolResult, ServiceError>;

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError>;
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
        Peer::<RoleClient>::call_tool(self, params).await
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        Peer::<RoleClient>::read_resource(self, params).await
    }
}

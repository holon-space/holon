//! Fetch layer for Claude Code sessions.
//!
//! `ClaudeSessionSource` is the transport seam: the sync engine only sees
//! typed [`SessionRecord`]s. `McpClaudeSessionSource` implements it over the
//! claude-history MCP server's per-project session-list resources.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::{ReadResourceRequestParam, ResourceContents};

use crate::mcp_call_surface::McpCallSurface;
use crate::mcp_sync_strategy::{expand_uri_template, json_array_to_records};

use super::boundary::{SessionRecord, parse_session_json};

/// URI template of the claude-history per-project session list (H8).
pub const SESSIONS_URI_TEMPLATE: &str = "claude-history://projects/{project_id}/sessions";

#[async_trait]
pub trait ClaudeSessionSource: Send + Sync {
    /// Fetch the current session list across all configured projects.
    async fn fetch_sessions(&self) -> anyhow::Result<Vec<SessionRecord>>;
}

/// MCP-backed source reading `claude-history://projects/{project_id}/sessions`
/// for each configured project id.
pub struct McpClaudeSessionSource {
    surface: Arc<dyn McpCallSurface>,
    /// claude-history project ids to enumerate (H8: cross-project enumeration
    /// is expensive; the configured set keeps polling bounded).
    project_ids: Vec<String>,
}

impl McpClaudeSessionSource {
    pub fn new(surface: Arc<dyn McpCallSurface>, project_ids: Vec<String>) -> Self {
        Self {
            surface,
            project_ids,
        }
    }
}

#[async_trait]
impl ClaudeSessionSource for McpClaudeSessionSource {
    async fn fetch_sessions(&self) -> anyhow::Result<Vec<SessionRecord>> {
        let mut sessions = Vec::new();
        for project_id in &self.project_ids {
            let uri = expand_uri_template(
                SESSIONS_URI_TEMPLATE,
                &HashMap::from([("project_id".to_string(), project_id.clone())]),
            )?;
            let result = self
                .surface
                .read_resource(ReadResourceRequestParam { uri: uri.clone() })
                .await
                .map_err(|e| anyhow::anyhow!("read_resource '{uri}' failed: {e}"))?;

            let text = result
                .contents
                .into_iter()
                .filter_map(|c| match c {
                    ResourceContents::TextResourceContents { text, .. } => Some(text),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");

            let parsed: serde_json::Value = serde_json::from_str(&text)
                .map_err(|e| anyhow::anyhow!("resource '{uri}' is not JSON: {e}"))?;
            let array = parsed
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("resource '{uri}' did not return a JSON array"))?;
            let records =
                json_array_to_records(array).map_err(|e| anyhow::anyhow!("resource '{uri}': {e}"))?;

            for obj in &records {
                sessions.push(
                    parse_session_json(obj)
                        .map_err(|e| anyhow::anyhow!("resource '{uri}': {e}"))?,
                );
            }
        }
        Ok(sessions)
    }
}

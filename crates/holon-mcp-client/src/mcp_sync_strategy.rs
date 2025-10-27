use std::borrow::Cow;
use std::collections::HashMap;

use async_trait::async_trait;
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParam, ReadResourceRequestParam, ResourceContents};
use rmcp::service::Peer;
use tracing::{debug, info};

use holon::core::datasource::{StreamPosition, SyncTokenStore};
use holon_api::Value;

use crate::mcp_sidecar::CursorConfig;

/// Convert a serde_json::Value to holon_api::Value, preserving nested objects as JSON text.
pub fn json_value_to_holon_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::Boolean(*b),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            } else if let Some(f) = n.as_f64() {
                Value::Float(f)
            } else {
                Value::String(n.to_string())
            }
        }
        serde_json::Value::String(s) => Value::String(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => Value::String(v.to_string()),
    }
}

/// A fetched record batch from an MCP server, with optional new cursor position.
pub struct FetchResult {
    /// JSON objects representing individual records.
    pub records: Vec<serde_json::Map<String, serde_json::Value>>,
    /// New cursor value if incremental sync is supported.
    pub new_cursor: Option<String>,
}

/// Abstracts over how records are fetched from an MCP server.
///
/// Two implementations:
/// - `ToolSync` — calls `peer.call_tool()` and extracts records via a JSON path
/// - `ResourceSync` — calls `peer.read_resource(uri)` and parses as JSON array
#[async_trait]
pub trait SyncStrategy: Send + Sync {
    /// Fetch records from the MCP server.
    async fn fetch_records(
        &self,
        peer: &Peer<RoleClient>,
        token_store: &dyn SyncTokenStore,
        token_key: &str,
    ) -> anyhow::Result<FetchResult>;

    /// URI to subscribe to for live updates, if supported.
    fn subscribe_uri(&self) -> Option<&str> {
        None
    }
}

/// Fetches records by calling an MCP tool (existing Todoist pattern).
pub struct ToolSync {
    pub list_tool: String,
    pub extract_path: String,
    pub list_params: HashMap<String, serde_json::Value>,
    pub cursor: Option<CursorConfig>,
}

#[async_trait]
impl SyncStrategy for ToolSync {
    async fn fetch_records(
        &self,
        peer: &Peer<RoleClient>,
        token_store: &dyn SyncTokenStore,
        token_key: &str,
    ) -> anyhow::Result<FetchResult> {
        let cursor_value = if let Some(ref cursor_config) = self.cursor {
            match token_store
                .load_token(token_key)
                .await
                .map_err(|e| anyhow::anyhow!("{e}"))?
            {
                Some(StreamPosition::Version(bytes)) => {
                    let cursor_str = String::from_utf8(bytes)?;
                    debug!(
                        "[ToolSync] Incremental sync, cursor param {}={}",
                        cursor_config.request_param, cursor_str
                    );
                    Some((cursor_config.request_param.clone(), cursor_str))
                }
                _ => None,
            }
        } else {
            None
        };

        let mut params: serde_json::Map<String, serde_json::Value> = self
            .list_params
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect();

        if let Some((param_name, cursor_str)) = &cursor_value {
            params.insert(
                param_name.clone(),
                serde_json::Value::String(cursor_str.clone()),
            );
        }

        info!("[ToolSync] Calling tool '{}'", self.list_tool);

        let result = peer
            .call_tool(CallToolRequestParam {
                name: Cow::Owned(self.list_tool.clone()),
                arguments: Some(params),
            })
            .await
            .map_err(|e| anyhow::anyhow!("MCP call_tool '{}' failed: {e}", self.list_tool))?;

        if result.is_error == Some(true) {
            let error_text: String = result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join("\n");
            anyhow::bail!("MCP tool '{}' returned error: {error_text}", self.list_tool);
        }

        let json_text: String = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("");

        let response: serde_json::Value = serde_json::from_str(&json_text)
            .map_err(|e| anyhow::anyhow!("Failed to parse tool response as JSON: {e}"))?;

        let records_json = response
            .get(&self.extract_path)
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!("Response missing '{}' array field", self.extract_path)
            })?;

        let records: Vec<serde_json::Map<String, serde_json::Value>> = records_json
            .iter()
            .filter_map(|r| r.as_object().cloned())
            .collect();

        let new_cursor = self.cursor.as_ref().and_then(|cc| {
            response
                .get(&cc.response_field)
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });

        info!(
            "[ToolSync] Got {} records from '{}'",
            records.len(),
            self.list_tool
        );

        Ok(FetchResult {
            records,
            new_cursor,
        })
    }
}

/// Fetches records by reading an MCP resource URI.
pub struct ResourceSync {
    pub uri: String,
}

#[async_trait]
impl SyncStrategy for ResourceSync {
    async fn fetch_records(
        &self,
        peer: &Peer<RoleClient>,
        _token_store: &dyn SyncTokenStore,
        _token_key: &str,
    ) -> anyhow::Result<FetchResult> {
        info!("[ResourceSync] Reading resource '{}'", self.uri);

        let result = peer
            .read_resource(ReadResourceRequestParam {
                uri: self.uri.clone(),
            })
            .await
            .map_err(|e| anyhow::anyhow!("MCP read_resource '{}' failed: {e}", self.uri))?;

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
            .map_err(|e| anyhow::anyhow!("Failed to parse resource response as JSON: {e}"))?;

        let records_array = parsed.as_array().ok_or_else(|| {
            anyhow::anyhow!("Resource '{}' did not return a JSON array", self.uri)
        })?;

        let records: Vec<serde_json::Map<String, serde_json::Value>> = records_array
            .iter()
            .filter_map(|r| r.as_object().cloned())
            .collect();

        info!(
            "[ResourceSync] Got {} records from '{}'",
            records.len(),
            self.uri
        );

        Ok(FetchResult {
            records,
            new_cursor: None,
        })
    }

    fn subscribe_uri(&self) -> Option<&str> {
        Some(&self.uri)
    }
}

/// Expand a URI template by replacing `{key}` placeholders with values from params.
///
/// Returns an error if any placeholder remains unresolved.
pub fn expand_uri_template(
    template: &str,
    params: &HashMap<String, String>,
) -> anyhow::Result<String> {
    let mut result = template.to_string();
    for (key, value) in params {
        result = result.replace(&format!("{{{key}}}"), value);
    }
    if let Some(start) = result.find('{') {
        if let Some(end) = result[start..].find('}') {
            let unresolved = &result[start + 1..start + end];
            anyhow::bail!("Unresolved URI template parameter '{{{unresolved}}}' in '{template}'");
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_uri_template_basic() {
        let mut params = HashMap::new();
        params.insert("project_id".to_string(), "my-project".to_string());
        let result =
            expand_uri_template("claude-history://projects/{project_id}/sessions", &params)
                .unwrap();
        assert_eq!(result, "claude-history://projects/my-project/sessions");
    }

    #[test]
    fn expand_uri_template_multiple_params() {
        let mut params = HashMap::new();
        params.insert("a".to_string(), "1".to_string());
        params.insert("b".to_string(), "2".to_string());
        let result = expand_uri_template("x/{a}/y/{b}/z", &params).unwrap();
        assert_eq!(result, "x/1/y/2/z");
    }

    #[test]
    fn expand_uri_template_no_params_needed() {
        let result = expand_uri_template("simple://uri", &HashMap::new()).unwrap();
        assert_eq!(result, "simple://uri");
    }

    #[test]
    fn expand_uri_template_unresolved_error() {
        let err = expand_uri_template("x/{missing}/y", &HashMap::new()).unwrap_err();
        assert!(err.to_string().contains("missing"));
    }
}

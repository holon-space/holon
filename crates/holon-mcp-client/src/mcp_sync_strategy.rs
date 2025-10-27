use std::borrow::Cow;
use std::collections::HashMap;

use async_trait::async_trait;
use rmcp::RoleClient;
use rmcp::model::{CallToolRequestParam, ReadResourceRequestParam, ResourceContents};
use rmcp::service::Peer;
use tracing::{Instrument, debug, info, info_span};

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
        let span = info_span!("resource_fetch", uri = %self.uri);
        async {
            info!("reading resource");

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

            info!(records = records.len(), "resource fetched");

            Ok(FetchResult {
                records,
                new_cursor: None,
            })
        }
        .instrument(span)
        .await
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

/// Inverse of `expand_uri_template`: given a template and a concrete URI,
/// extract the parameter values. Returns `None` if the URI doesn't match.
///
/// Example: `match_uri_template("x/{a}/y/{b}", "x/1/y/2")` → `Some({"a": "1", "b": "2"})`
pub fn match_uri_template(template: &str, uri: &str) -> Option<HashMap<String, String>> {
    let mut params = HashMap::new();
    let mut template_pos = 0;
    let mut uri_pos = 0;
    let template_bytes = template.as_bytes();
    let uri_bytes = uri.as_bytes();

    while template_pos < template_bytes.len() {
        if template_bytes[template_pos] == b'{' {
            // Extract param name
            let end = template[template_pos..].find('}')? + template_pos;
            let param_name = &template[template_pos + 1..end];
            template_pos = end + 1;

            // Find the next literal segment to know where the param value ends
            let value_end = if template_pos < template_bytes.len() {
                // Find the next literal character(s) in the URI
                let next_literal_end = if template_bytes[template_pos] == b'{' {
                    // Next segment is also a param — shouldn't happen in practice,
                    // but take a single path segment as the value
                    uri[uri_pos..]
                        .find('/')
                        .map(|i| uri_pos + i)
                        .unwrap_or(uri_bytes.len())
                } else {
                    // Find where the next literal segment starts in the template
                    let next_brace = template[template_pos..]
                        .find('{')
                        .map(|i| template_pos + i)
                        .unwrap_or(template_bytes.len());
                    let literal = &template[template_pos..next_brace];
                    // Find this literal in the remaining URI
                    uri[uri_pos..].find(literal).map(|i| uri_pos + i)?
                };
                next_literal_end
            } else {
                // Param is at the end of the template — consume rest of URI
                uri_bytes.len()
            };

            let value = &uri[uri_pos..value_end];
            params.insert(param_name.to_string(), value.to_string());
            uri_pos = value_end;
        } else {
            // Literal character — must match exactly
            if uri_pos >= uri_bytes.len() || uri_bytes[uri_pos] != template_bytes[template_pos] {
                return None;
            }
            template_pos += 1;
            uri_pos += 1;
        }
    }

    // Both template and URI must be fully consumed
    if uri_pos != uri_bytes.len() {
        return None;
    }

    Some(params)
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

    #[test]
    fn match_uri_template_basic() {
        let result = match_uri_template(
            "claude-history://sessions/{session_id}/messages",
            "claude-history://sessions/809ab486/messages",
        );
        let params = result.unwrap();
        assert_eq!(params.get("session_id").unwrap(), "809ab486");
        assert_eq!(params.len(), 1);
    }

    #[test]
    fn match_uri_template_multiple_params() {
        let result = match_uri_template("x/{a}/y/{b}/z", "x/1/y/2/z");
        let params = result.unwrap();
        assert_eq!(params.get("a").unwrap(), "1");
        assert_eq!(params.get("b").unwrap(), "2");
    }

    #[test]
    fn match_uri_template_param_at_end() {
        let result = match_uri_template("prefix/{id}", "prefix/abc-123");
        let params = result.unwrap();
        assert_eq!(params.get("id").unwrap(), "abc-123");
    }

    #[test]
    fn match_uri_template_no_params() {
        let result = match_uri_template("simple://uri", "simple://uri");
        assert_eq!(result.unwrap().len(), 0);
    }

    #[test]
    fn match_uri_template_mismatch() {
        assert!(match_uri_template("x/{a}/y", "x/1/z").is_none());
    }

    #[test]
    fn match_uri_template_trailing_param_captures_rest() {
        // Param at end of template captures everything remaining
        let result = match_uri_template("x/{a}", "x/1/extra").unwrap();
        assert_eq!(result.get("a").unwrap(), "1/extra");
    }

    #[test]
    fn match_uri_template_uri_too_short() {
        assert!(match_uri_template("x/{a}/y", "x/1").is_none());
    }

    #[test]
    fn match_uri_template_roundtrip() {
        let template = "claude-history://projects/{project_id}/sessions/{session_id}/messages";
        let mut params = HashMap::new();
        params.insert("project_id".to_string(), "my-project".to_string());
        params.insert("session_id".to_string(), "abc-123".to_string());
        let uri = expand_uri_template(template, &params).unwrap();
        let extracted = match_uri_template(template, &uri).unwrap();
        assert_eq!(extracted, params);
    }
}

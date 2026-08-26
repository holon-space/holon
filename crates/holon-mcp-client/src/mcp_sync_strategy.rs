use std::borrow::Cow;
use std::collections::HashMap;

use async_trait::async_trait;
use holon_api::StreamPosition;
use holon_api::Value;
use holon_core::SyncTokenStore;
use rmcp::model::CallToolRequestParam;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ResourceContents;
use serde::Deserialize;
use serde::Serialize;
use tracing::Instrument;
use tracing::debug;
use tracing::info;
use tracing::info_span;

use crate::mcp_call_surface::McpCallSurface;
use crate::mcp_sidecar::CursorConfig;

/// Convert a serde_json::Value to holon_api::Value.
pub fn json_value_to_holon_value(v: &serde_json::Value) -> Value {
    Value::from_json_value(v.clone())
}

/// A fetched record batch from an MCP server, with optional new cursor
/// position.
pub struct FetchResult {
    /// JSON objects representing individual records.
    pub records: Vec<serde_json::Map<String, serde_json::Value>>,
    /// New cursor value if incremental sync is supported.
    pub new_cursor: Option<String>,
}

/// Abstracts over how records are fetched from a data source.
///
/// Two implementations:
/// - `ToolSync` — calls `surface.call_tool()` and extracts records via a JSON
///   path
/// - `ResourceSync` — calls `surface.read_resource(uri)` and parses as JSON
///   array
///
/// The `surface` is any [`McpCallSurface`] — an rmcp `Peer` for the MCP
/// transports, or a `RestCallSurface` for the direct HTTP-API transport. The
/// fetch logic is identical across transports; only the leaf call surface
/// differs.
#[async_trait]
pub trait SyncStrategy: Send + Sync {
    /// Fetch records from the data source via `surface`.
    async fn fetch_records(
        &self,
        surface: &dyn McpCallSurface,
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
    /// Optional per-column field projection applied to each record after
    /// extraction: lifts nested JSON scalars into flat top-level columns (e.g.
    /// Google's `start.dateTime` → `start`). Empty ⇒ records are mapped by name
    /// unchanged.
    pub project: HashMap<String, Projection>,
}

/// A generic record-shaping rule (transport-agnostic): projects a nested JSON
/// value into a flat top-level column. Two primitives cover the common
/// nested-REST shapes without a bespoke transform:
///
/// - `path: [a.b, a.c]` — the FIRST present, non-null value along the listed
///   dotted paths wins (e.g. an event's `start.dateTime` for timed events,
///   falling back to `start.date` for all-day).
/// - `exists: a.b` — `1` when the dotted path is present and non-null, else `0`
///   (e.g. an all-day flag from the presence of `start.date`).
///
/// Exactly one primitive must be set; both/neither is a loud error at apply.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Projection {
    #[serde(default)]
    pub path: Vec<String>,
    #[serde(default)]
    pub exists: Option<String>,
}

impl Projection {
    /// Resolve this projection against a record, returning the value to store
    /// in the flat column.
    fn resolve(
        &self,
        record: &serde_json::Map<String, serde_json::Value>,
    ) -> anyhow::Result<serde_json::Value> {
        match (self.path.is_empty(), &self.exists) {
            (false, None) => {
                for p in &self.path {
                    if let Some(v) = lookup_dotted(record, p)
                        && !v.is_null()
                    {
                        return Ok(v.clone());
                    }
                }
                Ok(serde_json::Value::Null)
            }
            (true, Some(path)) => {
                let present = lookup_dotted(record, path).is_some_and(|v| !v.is_null());
                Ok(serde_json::Value::Number((present as i64).into()))
            }
            (true, None) => {
                anyhow::bail!("projection must set one of `path` or `exists`")
            }
            (false, Some(_)) => {
                anyhow::bail!("projection must set only one of `path` or `exists`, not both")
            }
        }
    }
}

/// Look up a dotted JSON path (`a.b.c`) within a record. Returns `None` if any
/// segment is missing or a non-object is traversed.
fn lookup_dotted<'a>(
    record: &'a serde_json::Map<String, serde_json::Value>,
    path: &str,
) -> Option<&'a serde_json::Value> {
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut cur = record.get(first)?;
    for seg in segments {
        cur = cur.as_object()?.get(seg)?;
    }
    Some(cur)
}

/// Apply a field projection to every column rule against `record`, computing
/// all values from the ORIGINAL record before writing (so a rule that reads
/// `start` and writes `start` sees the nested source, not a half-applied
/// column).
pub fn apply_projection(
    record: &mut serde_json::Map<String, serde_json::Value>,
    project: &HashMap<String, Projection>,
) -> anyhow::Result<()> {
    let mut writes: Vec<(String, serde_json::Value)> = Vec::with_capacity(project.len());
    for (col, spec) in project {
        writes.push((col.clone(), spec.resolve(record)?));
    }
    for (col, val) in writes {
        record.insert(col, val);
    }
    Ok(())
}

#[async_trait]
impl SyncStrategy for ToolSync {
    async fn fetch_records(
        &self,
        surface: &dyn McpCallSurface,
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

        let result = surface
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

        let response = crate::mcp_call_surface::extract_tool_response(&result)
            .map_err(|e| anyhow::anyhow!("Tool '{}' response: {e}", self.list_tool))?;

        let records_json = response
            .get(&self.extract_path)
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                anyhow::anyhow!("Response missing '{}' array field", self.extract_path)
            })?;

        let mut records = json_array_to_records(records_json)
            .map_err(|e| anyhow::anyhow!("Tool '{}' response: {e}", self.list_tool))?;

        if !self.project.is_empty() {
            for rec in &mut records {
                apply_projection(rec, &self.project).map_err(|e| {
                    anyhow::anyhow!("Tool '{}' field projection: {e}", self.list_tool)
                })?;
            }
        }

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
        surface: &dyn McpCallSurface,
        _: &dyn SyncTokenStore,
        _: &str,
    ) -> anyhow::Result<FetchResult> {
        let span = info_span!("resource_fetch", uri = %self.uri);
        async {
            info!("reading resource");

            let result = surface
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

            let records = json_array_to_records(records_array)
                .map_err(|e| anyhow::anyhow!("Resource '{}': {e}", self.uri))?;

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

/// Convert a JSON array of records into object maps, erroring on the first
/// non-object element. Silently dropping non-object elements would let a
/// server-side format change surface as "zero records fetched", which the
/// full-sync diff then interprets as "delete everything cached".
pub fn json_array_to_records(
    values: &[serde_json::Value],
) -> anyhow::Result<Vec<serde_json::Map<String, serde_json::Value>>> {
    values
        .iter()
        .enumerate()
        .map(|(i, v)| {
            v.as_object().cloned().ok_or_else(|| {
                let preview: String = v.to_string().chars().take(120).collect();
                anyhow::anyhow!("record array element {i} is not a JSON object: {preview}")
            })
        })
        .collect()
}

/// Expand a URI template by replacing `{key}` placeholders with values from
/// params.
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
    if let Some(start) = result.find('{')
        && let Some(end) = result[start..].find('}')
    {
        let unresolved = &result[start + 1..start + end];
        anyhow::bail!("Unresolved URI template parameter '{{{unresolved}}}' in '{template}'");
    }
    Ok(result)
}

/// Inverse of `expand_uri_template`: given a template and a concrete URI,
/// extract the parameter values. Returns `None` if the URI doesn't match.
///
/// Example: `match_uri_template("x/{a}/y/{b}", "x/1/y/2")` → `Some({"a": "1",
/// "b": "2"})`
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

                if template_bytes[template_pos] == b'{' {
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
                }
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

    fn rec(json: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
        json.as_object().unwrap().clone()
    }

    #[test]
    fn projection_lifts_nested_path_first_present() {
        // A timed Google event: start.dateTime present.
        let mut r = rec(serde_json::json!({
            "id": "e1",
            "start": {"dateTime": "2026-07-22T10:00:00+02:00", "timeZone": "Europe/Berlin"},
            "end": {"date": "2026-07-23"}
        }));
        let mut project = HashMap::new();
        project.insert(
            "start".to_string(),
            Projection {
                path: vec!["start.dateTime".into(), "start.date".into()],
                exists: None,
            },
        );
        project.insert(
            "end".to_string(),
            Projection {
                path: vec!["end.dateTime".into(), "end.date".into()],
                exists: None,
            },
        );
        project.insert(
            "all_day".to_string(),
            Projection {
                path: vec![],
                exists: Some("start.date".into()),
            },
        );
        apply_projection(&mut r, &project).unwrap();
        assert_eq!(r["start"], serde_json::json!("2026-07-22T10:00:00+02:00"));
        // end fell back to end.date (all-day-style end).
        assert_eq!(r["end"], serde_json::json!("2026-07-23"));
        // Not an all-day event (start.date absent) → 0.
        assert_eq!(r["all_day"], serde_json::json!(0));
    }

    #[test]
    fn projection_all_day_flag_and_date_fallback() {
        // An all-day event: start.date present, start.dateTime absent.
        let mut r = rec(serde_json::json!({
            "id": "e2",
            "start": {"date": "2026-07-22"},
            "end": {"date": "2026-07-23"}
        }));
        let mut project = HashMap::new();
        project.insert(
            "start".to_string(),
            Projection {
                path: vec!["start.dateTime".into(), "start.date".into()],
                exists: None,
            },
        );
        project.insert(
            "all_day".to_string(),
            Projection {
                path: vec![],
                exists: Some("start.date".into()),
            },
        );
        apply_projection(&mut r, &project).unwrap();
        assert_eq!(r["start"], serde_json::json!("2026-07-22"));
        assert_eq!(r["all_day"], serde_json::json!(1));
    }

    #[test]
    fn projection_absent_path_yields_null_not_error() {
        let mut r = rec(serde_json::json!({"id": "e3"}));
        let mut project = HashMap::new();
        project.insert(
            "start".to_string(),
            Projection {
                path: vec!["start.dateTime".into(), "start.date".into()],
                exists: None,
            },
        );
        apply_projection(&mut r, &project).unwrap();
        assert_eq!(r["start"], serde_json::Value::Null);
    }

    #[test]
    fn projection_both_primitives_fails_loud() {
        let mut r = rec(serde_json::json!({"a": 1}));
        let mut project = HashMap::new();
        project.insert(
            "x".to_string(),
            Projection {
                path: vec!["a".into()],
                exists: Some("a".into()),
            },
        );
        let err = apply_projection(&mut r, &project).unwrap_err();
        assert!(err.to_string().contains("only one"), "{err}");
    }

    #[test]
    fn projection_neither_primitive_fails_loud() {
        let mut r = rec(serde_json::json!({"a": 1}));
        let mut project = HashMap::new();
        project.insert(
            "x".to_string(),
            Projection {
                path: vec![],
                exists: None,
            },
        );
        let err = apply_projection(&mut r, &project).unwrap_err();
        assert!(err.to_string().contains("one of"), "{err}");
    }

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

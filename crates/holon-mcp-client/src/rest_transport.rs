//! Direct HTTP-API transport (`transport: rest`), a second transport behind the
//! same [`McpCallSurface`] seam the MCP transports use.
//!
//! Where the `http`/`child_process` transports reach a *server that speaks
//! MCP*, the `rest` transport reaches a plain HTTP/JSON API directly, using a
//! UTCP-manual-style description in the sidecar (base URL + per-tool endpoint
//! specs). A [`RestCallSurface`] makes that API look like an MCP call surface:
//! it translates `call_tool(name, args)` into an HTTP request per the manual
//! and packages the JSON response as a `CallToolResult`. Everything downstream
//! — the [`SyncStrategy`](crate::mcp_sync_strategy) fetch path, record
//! extraction, schema mapping, cache writing — is unchanged. One connector
//! engine, plural transports.
//!
//! Read-only for now: only `GET` calls are accepted. Write/mutation (and the
//! lease-governed external-effect question) are deliberately out of scope; a
//! non-GET method fails loud rather than silently issuing a mutating request.
//!
//! Secrets are never inlined: `base_url` and the optional auth header value are
//! `${VAR}`-expanded from the environment at startup (see
//! [`crate::integration_config`]), so the YAML references env/keychain names
//! only.

use std::collections::HashMap;

use async_trait::async_trait;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::Content;
use rmcp::model::ErrorData;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ReadResourceResult;
use rmcp::service::ServiceError;

use crate::mcp_call_surface::McpCallSurface;

/// A single HTTP endpoint, resolved from the sidecar's `transport.rest.calls`.
///
/// `path` and `query` values may contain `{arg}` placeholders that are filled
/// from the tool-call arguments at request time. This is distinct from the
/// `${VAR}` env syntax (which is expanded once at startup for `base_url`/auth):
/// `{arg}` is per-call data, `${VAR}` is a secret/config reference.
#[derive(Debug, Clone)]
pub struct RestCall {
    pub method: String,
    pub path: String,
    pub query: HashMap<String, String>,
    /// If set, a non-object JSON body (e.g. a bare array) is wrapped as
    /// `{ result_key: <body> }` so a `sync.extract_path` can select it. This is
    /// the response→block-shape adapter: REST APIs return arbitrary top-level
    /// shapes, the tool-response contract wants an object.
    pub result_key: Option<String>,
}

/// A UTCP-manual, resolved and ready to serve calls. Built by
/// [`crate::integration_config::IntegrationFileConfig::into_mcp_config_with`]
/// with `base_url` and the auth header value already `${VAR}`-expanded.
#[derive(Debug, Clone)]
pub struct RestManual {
    pub base_url: String,
    /// Optional `(header_name, header_value)` sent on every request. The value
    /// is already env-resolved — it may be a bearer token; never logged.
    pub auth_header: Option<(String, String)>,
    pub calls: HashMap<String, RestCall>,
}

/// [`McpCallSurface`] over a plain HTTP/JSON API described by a [`RestManual`].
pub struct RestCallSurface {
    manual: RestManual,
    client: reqwest::Client,
}

impl std::fmt::Debug for RestCallSurface {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestCallSurface")
            .field("base_url", &self.manual.base_url)
            .field("auth", &self.manual.auth_header.as_ref().map(|(h, _)| h))
            .field("calls", &self.manual.calls.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl RestCallSurface {
    pub fn new(manual: RestManual) -> Self {
        Self {
            manual,
            client: reqwest::Client::new(),
        }
    }

    /// Build the surface with a caller-supplied client (used by tests that need
    /// a specific timeout, and to avoid a per-surface client in hot paths).
    pub fn with_client(manual: RestManual, client: reqwest::Client) -> Self {
        Self { manual, client }
    }

    async fn do_call(&self, params: CallToolRequestParam) -> anyhow::Result<serde_json::Value> {
        let name = params.name.as_ref();
        let call = self.manual.calls.get(name).ok_or_else(|| {
            anyhow::anyhow!(
                "rest transport: no call named '{name}' in the manual (known: {:?})",
                self.manual.calls.keys().collect::<Vec<_>>()
            )
        })?;

        if !call.method.eq_ignore_ascii_case("GET") {
            anyhow::bail!(
                "rest transport: call '{name}' uses method '{}', but only GET is supported \
                 (write/mutation and lease semantics are out of scope)",
                call.method
            );
        }

        let args = params.arguments.unwrap_or_default();
        let path = fill_placeholders(&call.path, &args)
            .map_err(|e| anyhow::anyhow!("rest transport: call '{name}' path: {e}"))?;
        let url = format!(
            "{}/{}",
            self.manual.base_url.trim_end_matches('/'),
            path.trim_start_matches('/')
        );

        let mut query: Vec<(String, String)> = Vec::with_capacity(call.query.len());
        for (k, v) in &call.query {
            let filled = fill_placeholders(v, &args)
                .map_err(|e| anyhow::anyhow!("rest transport: call '{name}' query '{k}': {e}"))?;
            query.push((k.clone(), filled));
        }

        let mut req = self.client.get(&url).query(&query);
        if let Some((header, value)) = &self.manual.auth_header {
            req = req.header(header.as_str(), value.as_str());
        }

        let resp = req
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("rest transport: GET {url} failed: {e}"))?;
        let status = resp.status();
        let body = resp
            .text()
            .await
            .map_err(|e| anyhow::anyhow!("rest transport: reading body of GET {url}: {e}"))?;
        if !status.is_success() {
            let preview: String = body.chars().take(200).collect();
            anyhow::bail!("rest transport: GET {url} returned HTTP {status}: {preview}");
        }

        let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
            let preview: String = body.chars().take(200).collect();
            anyhow::anyhow!("rest transport: GET {url} body is not JSON: {e} (body: {preview})")
        })?;

        // Wrap non-object bodies so a `sync.extract_path` can select them.
        let structured = match &call.result_key {
            Some(key) => {
                let mut obj = serde_json::Map::new();
                obj.insert(key.clone(), parsed);
                serde_json::Value::Object(obj)
            }
            None => parsed,
        };
        Ok(structured)
    }
}

#[async_trait]
impl McpCallSurface for RestCallSurface {
    async fn call_tool(
        &self,
        params: CallToolRequestParam,
    ) -> Result<CallToolResult, ServiceError> {
        let name = params.name.to_string();
        match self.do_call(params).await {
            Ok(structured) => {
                // Also emit a text block so servers/consumers that ignore
                // `structured_content` still see the payload.
                let text =
                    serde_json::to_string(&structured).unwrap_or_else(|_| structured.to_string());
                Ok(CallToolResult {
                    content: vec![Content::text(text)],
                    structured_content: Some(structured),
                    is_error: None,
                    meta: None,
                })
            }
            Err(e) => Err(ServiceError::McpError(ErrorData::internal_error(
                format!("rest transport call '{name}': {e}"),
                None,
            ))),
        }
    }

    async fn read_resource(
        &self,
        params: ReadResourceRequestParam,
    ) -> Result<ReadResourceResult, ServiceError> {
        Err(ServiceError::McpError(ErrorData::internal_error(
            format!(
                "rest transport does not support MCP resources (requested '{}'); describe reads \
                 as GET calls under transport.rest.calls and use sync.list_tool instead",
                params.uri
            ),
            None,
        )))
    }
}

/// Replace `{name}` placeholders in `input` with argument values. Fails loud on
/// an unfilled placeholder — a missing argument must never silently produce a
/// malformed URL.
fn fill_placeholders(
    input: &str,
    args: &serde_json::Map<String, serde_json::Value>,
) -> anyhow::Result<String> {
    let mut out = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let end = after
            .find('}')
            .ok_or_else(|| anyhow::anyhow!("unterminated '{{' in '{input}'"))?;
        let key = &after[..end];
        let value = args.get(key).ok_or_else(|| {
            anyhow::anyhow!("no argument '{key}' supplied for placeholder in '{input}'")
        })?;
        let rendered = match value {
            serde_json::Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        out.push_str(&rendered);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), serde_json::Value::String(v.to_string())))
            .collect()
    }

    #[test]
    fn fill_placeholders_substitutes() {
        let out = fill_placeholders("/users/{id}/posts", &args(&[("id", "42")])).unwrap();
        assert_eq!(out, "/users/42/posts");
    }

    #[test]
    fn fill_placeholders_no_placeholders_is_identity() {
        let out = fill_placeholders("/posts", &args(&[])).unwrap();
        assert_eq!(out, "/posts");
    }

    #[test]
    fn fill_placeholders_missing_arg_fails_loud() {
        let err = fill_placeholders("/users/{id}", &args(&[])).unwrap_err();
        assert!(err.to_string().contains("no argument 'id'"), "{err}");
    }

    #[test]
    fn fill_placeholders_unterminated_fails_loud() {
        let err = fill_placeholders("/users/{id", &args(&[("id", "1")])).unwrap_err();
        assert!(err.to_string().contains("unterminated"), "{err}");
    }
}

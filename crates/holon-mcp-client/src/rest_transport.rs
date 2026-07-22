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
use std::sync::Arc;

use async_trait::async_trait;
use rmcp::model::CallToolRequestParam;
use rmcp::model::CallToolResult;
use rmcp::model::Content;
use rmcp::model::ErrorData;
use rmcp::model::ReadResourceRequestParam;
use rmcp::model::ReadResourceResult;
use rmcp::service::ServiceError;
use serde::Deserialize;
use serde::Serialize;
use tracing::warn;

use crate::mcp_call_surface::McpCallSurface;
use crate::rest_oauth2::OAuth2TokenProvider;
use crate::rest_oauth2::redact_url;

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
    /// How to decode the response body before extraction. `json` is the default
    /// (back-compat); `atom`/`rss` decode a syndication feed into the same
    /// record shape (see [`ResponseFormat`]).
    pub format: ResponseFormat,
    /// For `json`: if set, a non-object body (e.g. a bare array) is wrapped as
    /// `{ result_key: <body> }` so a `sync.extract_path` can select it. This is
    /// the response→block-shape adapter: REST APIs return arbitrary top-level
    /// shapes, the tool-response contract wants an object.
    ///
    /// For `atom`/`rss`: the decoded entry array is always wrapped under this
    /// key (a feed is inherently a collection); defaults to `entries` when
    /// unset.
    pub result_key: Option<String>,
    /// Optional response-token pagination (`json` only). When set, the call
    /// follows the provider's continuation token across pages, concatenating
    /// each page's item array, up to a hard, fail-loud `max_pages` bound.
    pub pagination: Option<Pagination>,
}

/// Response-token pagination for a `json` [`RestCall`] (e.g. Google's
/// `nextPageToken`). Generic: a token found at `next_token_path` in one
/// response is sent as the `token_param` query parameter on the next request;
/// each page's `items_path` array is concatenated. Bounded by `max_pages` — if
/// a continuation token still remains at the bound, the call FAILS LOUD rather
/// than looping unbounded (an unbounded external pull is never silently
/// truncated).
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pagination {
    /// JSON key of the array to concatenate across pages (e.g. `items`).
    pub items_path: String,
    /// JSON key holding the continuation token in the response (e.g.
    /// `nextPageToken`). Absent/empty ⇒ last page.
    pub next_token_path: String,
    /// Query-parameter name to send the continuation token as on the next
    /// request (e.g. `pageToken`).
    pub token_param: String,
    /// Hard upper bound on pages fetched. Exceeding it with a token still
    /// present is a loud error.
    pub max_pages: u32,
}

/// How a [`RestCall`] authenticates. Static header and OAuth2 are orthogonal
/// arms behind the same request path.
#[derive(Clone)]
pub enum RestAuth {
    /// No auth.
    None,
    /// A fixed `(header, value)` sent on every request. The value is
    /// env-resolved at startup and may be a bearer token; never logged.
    Static { header: String, value: String },
    /// OAuth2 refresh-token grant: a per-request `Authorization: Bearer` from a
    /// cached, auto-refreshed access token. Shared (`Arc`) so all calls of an
    /// integration share one token cache.
    OAuth2(Arc<OAuth2TokenProvider>),
}

impl std::fmt::Debug for RestAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact secret material in every arm.
        match self {
            RestAuth::None => write!(f, "None"),
            RestAuth::Static { header, .. } => {
                write!(f, "Static {{ header: {header:?}, value: <redacted> }}")
            }
            RestAuth::OAuth2(p) => write!(f, "OAuth2({p:?})"),
        }
    }
}

impl RestAuth {
    fn is_oauth2(&self) -> bool {
        matches!(self, RestAuth::OAuth2(_))
    }
}

/// How a [`RestCall`] response body is decoded into records.
///
/// `Json` is the default and back-compatible. `Atom`/`Rss` decode a syndication
/// feed (a fixed, standardized schema — RFC 4287 for Atom, RSS 2.0) into an
/// array of record objects with the same field-per-column contract the JSON
/// path uses, so the transport axis (mcp | rest) stays orthogonal to the body
/// codec (json | atom | rss). Parse-don't-validate: the codec is chosen at the
/// boundary and malformed input fails loud.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ResponseFormat {
    #[default]
    Json,
    Atom,
    Rss,
}

/// A UTCP-manual, resolved and ready to serve calls. Built by
/// [`crate::integration_config::IntegrationFileConfig::into_mcp_config_with`]
/// with `base_url` and any credentials already resolved from the environment /
/// keychain / files.
#[derive(Clone)]
pub struct RestManual {
    pub base_url: String,
    /// How requests authenticate ([`RestAuth::None`] for a public API).
    pub auth: RestAuth,
    pub calls: HashMap<String, RestCall>,
}

impl std::fmt::Debug for RestManual {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Manual (not derived) so the auth secret never lands in a debug/log
        // line even when a `McpTransport::Rest { .. }` is printed in a panic.
        f.debug_struct("RestManual")
            .field("base_url", &self.base_url)
            .field("auth", &self.auth)
            .field("calls", &self.calls.keys().collect::<Vec<_>>())
            .finish()
    }
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
            .field("auth", &self.manual.auth)
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

        // Pagination (json only) follows a continuation token across pages.
        if let Some(pg) = &call.pagination {
            if call.format != ResponseFormat::Json {
                anyhow::bail!(
                    "rest transport: call '{name}' sets pagination, which is only supported for \
                     `format: json` (atom/rss feeds are single-document)"
                );
            }
            return self.fetch_paginated(name, &url, &query, pg).await;
        }

        let body = self.get_with_auth_retry(name, &url, &query).await?;

        let structured = match call.format {
            ResponseFormat::Json => {
                let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
                    let preview: String = body.chars().take(200).collect();
                    anyhow::anyhow!(
                        "rest transport: GET {} body is not JSON: {e} (body: {preview})",
                        redact_url(&url)
                    )
                })?;
                // Wrap non-object bodies so a `sync.extract_path` can select them.
                match &call.result_key {
                    Some(key) => {
                        let mut obj = serde_json::Map::new();
                        obj.insert(key.clone(), parsed);
                        serde_json::Value::Object(obj)
                    }
                    None => parsed,
                }
            }
            ResponseFormat::Atom | ResponseFormat::Rss => {
                let entries = parse_feed(call.format, &body).map_err(|e| {
                    anyhow::anyhow!("rest transport: GET {}: {e}", redact_url(&url))
                })?;
                // A feed is inherently a collection; always wrap under the key so
                // `sync.extract_path` can select it (default `entries`).
                let key = call.result_key.as_deref().unwrap_or("entries");
                let mut obj = serde_json::Map::new();
                obj.insert(key.to_string(), serde_json::Value::Array(entries));
                serde_json::Value::Object(obj)
            }
        };
        Ok(structured)
    }

    /// Issue one GET, attaching auth per [`RestAuth`]. `force_token_refresh`
    /// forces a fresh OAuth2 access token (used on a 401 retry). Returns the
    /// response status and body; never includes a bearer token in any error.
    async fn send_get(
        &self,
        url: &str,
        query: &[(String, String)],
        force_token_refresh: bool,
    ) -> anyhow::Result<(reqwest::StatusCode, String)> {
        let mut req = self.client.get(url).query(query);
        req = match &self.manual.auth {
            RestAuth::None => req,
            RestAuth::Static { header, value } => req.header(header.as_str(), value.as_str()),
            RestAuth::OAuth2(provider) => {
                let token = if force_token_refresh {
                    provider.force_refresh().await?
                } else {
                    provider.access_token().await?
                };
                // The bearer token lives only in this header — never in the URL,
                // query, or any error string below.
                req.header("Authorization", format!("Bearer {token}"))
            }
        };
        let resp = req.send().await.map_err(|e| {
            anyhow::anyhow!(
                "rest transport: GET {} failed: {}",
                redact_url(url),
                e.without_url()
            )
        })?;
        let status = resp.status();
        let body = resp.text().await.map_err(|e| {
            anyhow::anyhow!(
                "rest transport: reading body of GET {}: {}",
                redact_url(url),
                e.without_url()
            )
        })?;
        Ok((status, body))
    }

    /// A GET that, for OAuth2 auth, transparently refreshes the token once on a
    /// 401 and retries — recovering from a token the server rejected before we
    /// thought it expired. Non-2xx (after any retry) fails loud.
    async fn get_with_auth_retry(
        &self,
        name: &str,
        url: &str,
        query: &[(String, String)],
    ) -> anyhow::Result<String> {
        let (status, body) = self.send_get(url, query, false).await?;
        if status == reqwest::StatusCode::UNAUTHORIZED && self.manual.auth.is_oauth2() {
            warn!(
                "rest transport: call '{name}' GET {} returned 401 — refreshing OAuth2 token and \
                 retrying once",
                redact_url(url)
            );
            let (status2, body2) = self.send_get(url, query, true).await?;
            if !status2.is_success() {
                let preview: String = body2.chars().take(200).collect();
                anyhow::bail!(
                    "rest transport: call '{name}' GET {} still failed after token refresh: HTTP \
                     {status2}: {preview}",
                    redact_url(url)
                );
            }
            return Ok(body2);
        }
        if !status.is_success() {
            let preview: String = body.chars().take(200).collect();
            anyhow::bail!(
                "rest transport: call '{name}' GET {} returned HTTP {status}: {preview}",
                redact_url(url)
            );
        }
        Ok(body)
    }

    /// Follow a continuation token across pages, concatenating each page's
    /// `items_path` array, bounded by `max_pages` (fail loud if a token still
    /// remains at the bound). Returns the LAST page's object with `items_path`
    /// replaced by the accumulated array, so a `sync.extract_path` selecting
    /// `items_path` sees every record.
    async fn fetch_paginated(
        &self,
        name: &str,
        url: &str,
        base_query: &[(String, String)],
        pg: &Pagination,
    ) -> anyhow::Result<serde_json::Value> {
        if pg.max_pages == 0 {
            anyhow::bail!("rest transport: call '{name}' pagination.max_pages must be >= 1");
        }
        let mut items: Vec<serde_json::Value> = Vec::new();
        let mut token: Option<String> = None;

        for _page in 0..pg.max_pages {
            let mut query = base_query.to_vec();
            if let Some(t) = &token {
                query.push((pg.token_param.clone(), t.clone()));
            }
            let body = self.get_with_auth_retry(name, url, &query).await?;
            let parsed: serde_json::Value = serde_json::from_str(&body).map_err(|e| {
                let preview: String = body.chars().take(200).collect();
                anyhow::anyhow!(
                    "rest transport: GET {} body is not JSON: {e} (body: {preview})",
                    redact_url(url)
                )
            })?;
            let obj = parsed.as_object().ok_or_else(|| {
                anyhow::anyhow!(
                    "rest transport: call '{name}' paginated response must be a JSON object with \
                     an '{}' array",
                    pg.items_path
                )
            })?;
            let page_items = obj
                .get(&pg.items_path)
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "rest transport: call '{name}' paginated response missing '{}' array",
                        pg.items_path
                    )
                })?;
            items.extend(page_items.iter().cloned());

            let next = obj
                .get(&pg.next_token_path)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string());

            if next.is_none() {
                // Last page: return this page's object with the accumulated
                // items so a `sync.extract_path` on `items_path` sees them all.
                let mut out = obj.clone();
                out.insert(pg.items_path.clone(), serde_json::Value::Array(items));
                return Ok(serde_json::Value::Object(out));
            }
            token = next;
        }

        anyhow::bail!(
            "rest transport: call '{name}' pagination exceeded max_pages={} (a continuation token \
             remained at '{}'); refusing to loop unbounded — raise max_pages or narrow the query",
            pg.max_pages,
            pg.next_token_path
        )
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

/// Replace `{name}` placeholders in `input`. A placeholder is resolved as:
///
/// 1. a **now-token** (`{now}`, `{now-1d}`, `{now+14d}`, `{now+2h}`, …) →
///    rendered as an RFC 3339 UTC timestamp at request time, or
/// 2. an **argument** from `args` (path/query data from the tool call).
///
/// Fails loud on an unfilled placeholder — a missing argument or a malformed
/// now-token must never silently produce a malformed URL. (This is distinct
/// from `${VAR}`, which is a startup-time secret/config reference.)
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
        let rendered = if let Some(ts) = render_now_token(key)? {
            ts
        } else {
            let value = args.get(key).ok_or_else(|| {
                anyhow::anyhow!("no argument '{key}' supplied for placeholder in '{input}'")
            })?;
            match value {
                serde_json::Value::String(s) => s.clone(),
                other => other.to_string(),
            }
        };
        out.push_str(&rendered);
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    Ok(out)
}

/// Render a now-relative time token to an RFC 3339 UTC timestamp
/// (`YYYY-MM-DDThh:mm:ssZ`), or `None` if `key` is not a now-token.
///
/// Grammar: `now` optionally followed by a signed offset `('+'|'-') <int>
/// ('d'|'h'|'m'|'s')`, e.g. `now`, `now-1d`, `now+14d`, `now+30m`. A malformed
/// offset on an otherwise `now…` key is a loud error (never silently treated as
/// a literal argument name). Uses the project clock seam so it is deterministic
/// under test.
fn render_now_token(key: &str) -> anyhow::Result<Option<String>> {
    let offset = if key == "now" {
        chrono::Duration::zero()
    } else if let Some(spec) = key.strip_prefix("now") {
        // spec must start with '+' or '-'; otherwise this isn't a now-token
        // (e.g. an arg literally named "nowhere").
        let sign = match spec.chars().next() {
            Some('+') => 1,
            Some('-') => -1,
            _ => return Ok(None),
        };
        let rest = &spec[1..];
        let unit = rest.chars().last().ok_or_else(|| {
            anyhow::anyhow!("malformed now-token '{{{key}}}': missing unit (expected d/h/m/s)")
        })?;
        let n: i64 = rest[..rest.len() - 1].parse().map_err(|_| {
            anyhow::anyhow!("malformed now-token '{{{key}}}': expected now±<int><d|h|m|s>")
        })?;
        let magnitude = match unit {
            'd' => chrono::Duration::days(n),
            'h' => chrono::Duration::hours(n),
            'm' => chrono::Duration::minutes(n),
            's' => chrono::Duration::seconds(n),
            other => anyhow::bail!(
                "malformed now-token '{{{key}}}': unknown unit '{other}' (expected d/h/m/s)"
            ),
        };
        magnitude * sign as i32
    } else {
        return Ok(None);
    };

    let now =
        chrono::DateTime::<chrono::Utc>::from_timestamp_millis(holon_api::clock::now_millis())
            .ok_or_else(|| anyhow::anyhow!("clock returned an out-of-range timestamp"))?;
    let at = now + offset;
    Ok(Some(at.format("%Y-%m-%dT%H:%M:%SZ").to_string()))
}

// ---------------------------------------------------------------------------
// Syndication-feed codecs (Atom RFC 4287 / RSS 2.0)
//
// Both decode a standardized, fixed-schema XML document into the SAME record
// shape the JSON path produces — a Vec of objects with stable keys
// (id/title/updated/author/link/content) — so a feed maps onto block-shape
// columns exactly like a JSON API. Parse-don't-validate: the root element is
// asserted to be a feed at the boundary; anything else fails loud. An empty
// feed (zero entries) is legitimate and yields an empty array.
// ---------------------------------------------------------------------------

fn parse_feed(format: ResponseFormat, body: &str) -> anyhow::Result<Vec<serde_json::Value>> {
    let doc = roxmltree::Document::parse(body).map_err(|e| {
        let preview: String = body.chars().take(200).collect();
        anyhow::anyhow!("malformed XML feed: {e} (body: {preview})")
    })?;
    let root = doc.root_element().tag_name().name().to_string();

    let records = match format {
        ResponseFormat::Atom => {
            if root != "feed" {
                anyhow::bail!("expected an Atom <feed> root element, got <{root}>");
            }
            doc.descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "entry")
                .map(atom_entry_to_record)
                .collect()
        }
        ResponseFormat::Rss => {
            if root != "rss" {
                anyhow::bail!("expected an RSS <rss> root element, got <{root}>");
            }
            doc.descendants()
                .filter(|n| n.is_element() && n.tag_name().name() == "item")
                .map(rss_item_to_record)
                .collect()
        }
        ResponseFormat::Json => unreachable!("parse_feed is only called for atom/rss"),
    };
    Ok(records)
}

/// Map an Atom `<entry>` (RFC 4287) to a record object.
fn atom_entry_to_record(entry: roxmltree::Node<'_, '_>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    put(&mut obj, "id", child_text(entry, "id"));
    put(&mut obj, "title", child_text(entry, "title"));
    put(&mut obj, "updated", child_text(entry, "updated"));
    // <author> is an element carrying a <name> child.
    let author = entry
        .children()
        .find(|c| c.is_element() && c.tag_name().name() == "author")
        .and_then(|a| child_text(a, "name"));
    put(&mut obj, "author", author);
    put(&mut obj, "link", atom_link(entry));
    // Prefer full <content>, fall back to <summary>.
    put(
        &mut obj,
        "content",
        child_text(entry, "content").or_else(|| child_text(entry, "summary")),
    );
    serde_json::Value::Object(obj)
}

/// Atom links are `<link href="..." rel="..."/>`; prefer `alternate` (or no
/// rel).
fn atom_link(entry: roxmltree::Node<'_, '_>) -> Option<String> {
    let links: Vec<_> = entry
        .children()
        .filter(|c| c.is_element() && c.tag_name().name() == "link")
        .collect();
    links
        .iter()
        .find(|l| l.attribute("rel").is_none_or(|r| r == "alternate"))
        .or_else(|| links.first())
        .and_then(|l| l.attribute("href"))
        .map(|s| s.to_string())
}

/// Map an RSS 2.0 `<item>` to the same record shape as
/// [`atom_entry_to_record`].
fn rss_item_to_record(item: roxmltree::Node<'_, '_>) -> serde_json::Value {
    let mut obj = serde_json::Map::new();
    // guid is the stable id; fall back to the link.
    put(
        &mut obj,
        "id",
        child_text(item, "guid").or_else(|| child_text(item, "link")),
    );
    put(&mut obj, "title", child_text(item, "title"));
    put(&mut obj, "updated", child_text(item, "pubDate"));
    // <author>, or the widely-used <dc:creator> (local name "creator").
    put(
        &mut obj,
        "author",
        child_text(item, "author").or_else(|| child_text(item, "creator")),
    );
    // In RSS the link is element text (not an attribute).
    put(&mut obj, "link", child_text(item, "link"));
    // <description>, or <content:encoded> (local name "encoded").
    put(
        &mut obj,
        "content",
        child_text(item, "description").or_else(|| child_text(item, "encoded")),
    );
    serde_json::Value::Object(obj)
}

/// Text of the first child element with the given local name
/// (namespace-agnostic), trimmed; `None` when absent or empty.
fn child_text(node: roxmltree::Node<'_, '_>, local: &str) -> Option<String> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == local)
        .and_then(|c| c.text())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn put(obj: &mut serde_json::Map<String, serde_json::Value>, key: &str, val: Option<String>) {
    if let Some(v) = val {
        obj.insert(key.to_string(), serde_json::Value::String(v));
    }
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

    // --- now-token rendering ---------------------------------------------

    fn parse_rfc3339(s: &str) -> chrono::DateTime<chrono::Utc> {
        chrono::DateTime::parse_from_rfc3339(s)
            .unwrap_or_else(|e| panic!("'{s}' is not RFC3339: {e}"))
            .with_timezone(&chrono::Utc)
    }

    #[test]
    fn now_token_renders_rfc3339_utc() {
        let out = fill_placeholders("timeMin={now}", &args(&[])).unwrap();
        let ts = out.strip_prefix("timeMin=").unwrap();
        assert!(ts.ends_with('Z'), "expected UTC 'Z' suffix: {ts}");
        let parsed = parse_rfc3339(ts);
        let now = chrono::Utc::now();
        assert!(
            (parsed - now).num_seconds().abs() < 120,
            "now-token far from now: {ts}"
        );
    }

    #[test]
    fn now_token_applies_signed_offsets() {
        let out = fill_placeholders("a={now-1d}&b={now+14d}&c={now+30m}", &args(&[])).unwrap();
        // Parse the three timestamps back out.
        let parts: Vec<&str> = out.split(['&', '=']).collect();
        let minus1d = parse_rfc3339(parts[1]);
        let plus14d = parse_rfc3339(parts[3]);
        let plus30m = parse_rfc3339(parts[5]);
        assert!(
            plus14d > plus30m && plus30m > minus1d,
            "ordering wrong: {out}"
        );
        // ~15 days between now-1d and now+14d.
        assert_eq!((plus14d - minus1d).num_days(), 15);
    }

    #[test]
    fn now_token_malformed_offset_fails_loud() {
        let err = fill_placeholders("t={now+xyz}", &args(&[])).unwrap_err();
        assert!(err.to_string().contains("now-token"), "{err}");
    }

    #[test]
    fn non_now_key_is_treated_as_arg_not_time() {
        // A literal arg whose name merely starts with "now" (no +/- offset) is
        // still an argument lookup, never a time token.
        let out = fill_placeholders("/{nowhere}", &args(&[("nowhere", "x")])).unwrap();
        assert_eq!(out, "/x");
    }

    const ATOM_SAMPLE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<feed xmlns="http://www.w3.org/2005/Atom">
  <title>Example Feed</title>
  <entry>
    <id>urn:uuid:1</id>
    <title>First post</title>
    <updated>2026-07-12T10:00:00Z</updated>
    <author><name>Ada</name></author>
    <link rel="alternate" href="https://example.com/1"/>
    <content type="html">Body one</content>
  </entry>
  <entry>
    <id>urn:uuid:2</id>
    <title>Second post</title>
    <updated>2026-07-11T09:00:00Z</updated>
    <link href="https://example.com/2"/>
    <summary>Just a summary</summary>
  </entry>
</feed>"#;

    const RSS_SAMPLE: &str = r#"<?xml version="1.0"?>
<rss version="2.0" xmlns:dc="http://purl.org/dc/elements/1.1/">
  <channel>
    <title>Example Channel</title>
    <item>
      <guid>post-1</guid>
      <title>RSS first</title>
      <pubDate>Sat, 12 Jul 2026 10:00:00 GMT</pubDate>
      <dc:creator>Grace</dc:creator>
      <link>https://example.com/r1</link>
      <description>RSS body one</description>
    </item>
    <item>
      <title>RSS second</title>
      <link>https://example.com/r2</link>
    </item>
  </channel>
</rss>"#;

    #[test]
    fn parse_atom_extracts_entries() {
        let entries = parse_feed(ResponseFormat::Atom, ATOM_SAMPLE).unwrap();
        assert_eq!(entries.len(), 2);
        let e0 = &entries[0];
        assert_eq!(e0["id"], "urn:uuid:1");
        assert_eq!(e0["title"], "First post");
        assert_eq!(e0["updated"], "2026-07-12T10:00:00Z");
        assert_eq!(e0["author"], "Ada");
        assert_eq!(e0["link"], "https://example.com/1");
        assert_eq!(e0["content"], "Body one");
        // Second entry: no author/content — falls back to <summary>, link has no rel.
        let e1 = &entries[1];
        assert_eq!(e1["content"], "Just a summary");
        assert_eq!(e1["link"], "https://example.com/2");
        assert!(e1.get("author").is_none());
    }

    #[test]
    fn parse_rss_extracts_items() {
        let items = parse_feed(ResponseFormat::Rss, RSS_SAMPLE).unwrap();
        assert_eq!(items.len(), 2);
        let i0 = &items[0];
        assert_eq!(i0["id"], "post-1");
        assert_eq!(i0["title"], "RSS first");
        assert_eq!(i0["updated"], "Sat, 12 Jul 2026 10:00:00 GMT");
        assert_eq!(i0["author"], "Grace"); // dc:creator
        assert_eq!(i0["link"], "https://example.com/r1");
        assert_eq!(i0["content"], "RSS body one");
        // Second item: no guid → id falls back to link.
        assert_eq!(items[1]["id"], "https://example.com/r2");
    }

    #[test]
    fn parse_feed_malformed_xml_fails_loud() {
        let err = parse_feed(ResponseFormat::Atom, "<feed><entry>unclosed").unwrap_err();
        assert!(err.to_string().contains("malformed XML"), "{err}");
    }

    #[test]
    fn parse_feed_wrong_root_fails_loud() {
        let err = parse_feed(ResponseFormat::Atom, "<html><body>nope</body></html>").unwrap_err();
        assert!(err.to_string().contains("expected an Atom <feed>"), "{err}");
        let err = parse_feed(ResponseFormat::Rss, ATOM_SAMPLE).unwrap_err();
        assert!(err.to_string().contains("expected an RSS <rss>"), "{err}");
    }

    #[test]
    fn empty_feed_is_legitimate() {
        let empty = r#"<feed xmlns="http://www.w3.org/2005/Atom"><title>x</title></feed>"#;
        let entries = parse_feed(ResponseFormat::Atom, empty).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn response_format_defaults_to_json() {
        assert_eq!(ResponseFormat::default(), ResponseFormat::Json);
        let f: ResponseFormat = serde_yaml::from_str("atom").unwrap();
        assert_eq!(f, ResponseFormat::Atom);
    }
}

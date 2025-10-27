//! MCP Foreign Data Wrapper — queries external MCP servers through Turso's FDW API.
//!
//! Translates SQL WHERE constraints into MCP tool parameters via a declarative
//! `FilterMapping`, then fetches results through `peer.call_tool()`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;

use rmcp::RoleClient;
use rmcp::model::CallToolRequestParam;
use rmcp::service::Peer;
use serde::{Deserialize, Serialize};
use tracing::info;
use turso_core::Connection as CoreConnection;
use turso_core::foreign::{ForeignCursor, ForeignDataWrapper, KeyColumn, PushedConstraint};
use turso_core::{LimboError, Value};
use turso_ext::ConstraintOp;

// ============================================================================
// YAML sidecar config types
// ============================================================================

/// Virtual table configuration for an entity in the YAML sidecar.
///
/// Supports two fetch modes:
/// - **Tool-based**: `search_tool` + `extract_path` — calls an MCP tool with filter pushdown
/// - **Resource-based**: `list_resource` — reads an MCP resource URI (no pushdown, full fetch)
///
/// Exactly one of `search_tool` or `list_resource` must be set.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VtableConfig {
    /// MCP tool name for search/list queries (tool-based mode).
    #[serde(default)]
    pub search_tool: Option<String>,
    /// JSON key in the tool response containing the records array.
    /// Required when `search_tool` is set.
    #[serde(default)]
    pub extract_path: Option<String>,
    /// MCP resource URI to read for listing records (resource-based mode).
    /// The response must be a JSON array of objects.
    #[serde(default)]
    pub list_resource: Option<String>,
    /// Parameters to expand in the resource URI template.
    #[serde(default)]
    pub uri_params: HashMap<String, UriParamValue>,
    /// If true, write fetched results back to the cache table (opportunistic caching).
    #[serde(default)]
    pub write_through: bool,
    /// Maps column names to MCP tool parameters with supported operators.
    /// Only meaningful for tool-based mode.
    #[serde(default)]
    pub filter_mapping: HashMap<String, FilterColumnConfig>,
}

/// Per-column filter pushdown configuration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FilterColumnConfig {
    /// MCP tool parameter name (e.g., "from" for a from_address column).
    pub param: String,
    /// SQL operators this column supports for server-side filtering.
    #[serde(default = "default_ops")]
    pub ops: Vec<String>,
    /// If true, queries without this column in WHERE return an error.
    #[serde(default)]
    pub required: bool,
}

fn default_ops() -> Vec<String> {
    vec!["eq".to_string()]
}

/// A URI template parameter value — either static, dynamic (required from WHERE),
/// or dynamic with a fallback enumeration from another entity.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UriParamValue {
    /// Structured: dynamic param with fallback enumeration.
    Dynamic(DynamicUriParam),
    /// Plain string: empty = required from WHERE, non-empty = static.
    Static(String),
}

impl UriParamValue {
    /// Static non-empty value that gets baked into the URI at creation time.
    pub fn as_static(&self) -> Option<&str> {
        match self {
            UriParamValue::Static(s) if !s.is_empty() => Some(s),
            _ => None,
        }
    }

    /// Whether this param must be resolved dynamically (from WHERE or fallback).
    pub fn is_dynamic(&self) -> bool {
        self.as_static().is_none()
    }
}

/// Dynamic URI param with a fallback: when WHERE doesn't provide the value,
/// enumerate all values from the referenced entity's field.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DynamicUriParam {
    pub enumerate_from: EnumerateFrom,
}

/// Reference to another entity's field for fallback enumeration.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EnumerateFrom {
    /// Entity name (without prefix), e.g. `"session"`.
    pub entity: String,
    /// Field to enumerate, e.g. `"id"`.
    pub field: String,
}

/// Resolved fallback for a dynamic URI param — the SQL query to enumerate values.
/// Pre-computed at construction time from `EnumerateFrom` + entity prefix.
#[derive(Debug, Clone)]
struct ResolvedFallback {
    /// SQL to enumerate fallback values, e.g. `SELECT id FROM cc_session`.
    enumerate_sql: String,
}

/// How the FDW fetches data from the MCP server.
#[derive(Debug, Clone)]
enum FetchMode {
    /// Call an MCP tool with optional filter pushdown.
    Tool {
        search_tool: String,
        extract_path: String,
    },
    /// Read an MCP resource URI (returns JSON array, no pushdown).
    Resource { uri: String },
    /// Read an MCP resource URI with dynamic template params resolved from WHERE constraints.
    ResourceTemplate {
        template: String,
        /// Static params baked in from config (non-empty values).
        default_params: HashMap<String, String>,
        /// Param name → fallback SQL for params that have `enumerate_from`.
        /// When WHERE doesn't provide the param, run this SQL to get all values.
        fallbacks: HashMap<String, ResolvedFallback>,
    },
}

fn parse_constraint_op(s: &str) -> Option<ConstraintOp> {
    match s.to_lowercase().as_str() {
        "eq" | "=" => Some(ConstraintOp::Eq),
        "ne" | "!=" | "<>" => Some(ConstraintOp::Ne),
        "lt" | "<" => Some(ConstraintOp::Lt),
        "le" | "<=" => Some(ConstraintOp::Le),
        "gt" | ">" => Some(ConstraintOp::Gt),
        "ge" | ">=" => Some(ConstraintOp::Ge),
        "like" => Some(ConstraintOp::Like),
        "glob" => Some(ConstraintOp::Glob),
        "match" => Some(ConstraintOp::Match),
        "regexp" => Some(ConstraintOp::Regexp),
        _ => None,
    }
}

// ============================================================================
// McpForeignDataWrapper
// ============================================================================

/// A [`ForeignDataWrapper`] that queries an MCP server via tool calls.
///
/// Constructed from the YAML sidecar `vtable:` config and the live MCP peer.
/// Registered at startup via `conn.register_foreign_table()`.
#[derive(Debug)]
pub struct McpForeignDataWrapper {
    /// Live connection to the MCP server.
    peer: Arc<Peer<RoleClient>>,
    /// How to fetch data — tool call or resource read.
    fetch_mode: FetchMode,
    /// CREATE TABLE DDL for schema declaration.
    schema_sql: String,
    /// Declarative pushdown metadata.
    key_columns: Vec<KeyColumn>,
    /// Maps column_index → MCP tool parameter name.
    column_to_param: HashMap<u32, String>,
    /// Schema column names in order — used to align JSON response fields
    /// with the positional column indices expected by Turso.
    column_names: Vec<String>,
    /// ID column name (e.g., "id") and scheme prefix (e.g., "cc_session").
    /// When set, the ID column value is prefixed: `{scheme}:{raw_value}`.
    id_scheme: Option<(String, String)>,
    /// If set, fetched rows are written to this cache table via INSERT OR REPLACE.
    cache_table: Option<String>,
    /// Tokio runtime handle for async→sync bridge in filter().
    runtime: tokio::runtime::Handle,
}

/// Build key columns, column→param mapping, and schema DDL from sidecar config.
/// Extracted for testability (doesn't require a live MCP Peer).
///
/// Key columns come from two sources:
/// 1. Explicit `filter_mapping` entries (tool-based pushdown)
/// 2. Empty `uri_params` values (resource template pushdown — column name must match param name)
fn build_fdw_metadata(
    table_name: &str,
    columns: &[(String, String)],
    vtable_config: &VtableConfig,
) -> (Vec<KeyColumn>, HashMap<u32, String>, String, Vec<String>) {
    let schema_sql = format!(
        "CREATE TABLE {}({})",
        table_name,
        columns
            .iter()
            .map(|(name, ty)| format!("{name} {ty}"))
            .collect::<Vec<_>>()
            .join(", ")
    );

    let mut key_columns = Vec::new();
    let mut column_to_param = HashMap::new();

    for (col_idx, (col_name, _col_type)) in columns.iter().enumerate() {
        // Source 1: explicit filter_mapping
        if let Some(filter_config) = vtable_config.filter_mapping.get(col_name) {
            let ops: Vec<ConstraintOp> = filter_config
                .ops
                .iter()
                .filter_map(|s| parse_constraint_op(s))
                .collect();

            if ops.is_empty() {
                continue;
            }

            let mut kc = KeyColumn::new(col_name.clone(), col_idx as u32, ops);
            if filter_config.required {
                kc = kc.required();
            }
            column_to_param.insert(col_idx as u32, filter_config.param.clone());
            key_columns.push(kc);
        }
        // Source 2: dynamic URI template params (column name == param name, Eq-only)
        // Required only if there's no enumerate_from fallback.
        else if let Some(param_val) = vtable_config.uri_params.get(col_name) {
            if param_val.is_dynamic() {
                let has_fallback = matches!(param_val, UriParamValue::Dynamic(_));
                let mut kc =
                    KeyColumn::new(col_name.clone(), col_idx as u32, vec![ConstraintOp::Eq]);
                if !has_fallback {
                    kc = kc.required();
                }
                column_to_param.insert(col_idx as u32, col_name.clone());
                key_columns.push(kc);
            }
        }
    }

    let column_names: Vec<String> = columns.iter().map(|(name, _)| name.clone()).collect();
    (key_columns, column_to_param, schema_sql, column_names)
}

impl McpForeignDataWrapper {
    /// Build from YAML sidecar config + live MCP peer.
    ///
    /// `table_name` is the SQL table name (e.g., "gmail_email").
    /// `columns` are the schema columns (name, sql_type) pairs.
    /// Build from YAML sidecar config + live MCP peer.
    ///
    /// `id_scheme` is `Some((id_column, scheme_prefix))` to prefix ID values
    /// with `{scheme_prefix}:{raw}` (matching McpSyncEngine's convention).
    /// `cache_table` is the name of the local BTree table to write through to.
    /// `entity_prefix` is needed to resolve `enumerate_from` entity references
    /// to actual SQL table names (e.g. `"cc_"` + `"session"` → `"cc_session"`).
    pub fn new(
        table_name: &str,
        columns: &[(String, String)],
        vtable_config: &VtableConfig,
        peer: Arc<Peer<RoleClient>>,
        id_scheme: Option<(String, String)>,
        cache_table: Option<String>,
        runtime: tokio::runtime::Handle,
        entity_prefix: Option<&str>,
    ) -> Self {
        let (key_columns, column_to_param, schema_sql, column_names) =
            build_fdw_metadata(table_name, columns, vtable_config);

        let fetch_mode = if let Some(ref tool) = vtable_config.search_tool {
            FetchMode::Tool {
                search_tool: tool.clone(),
                extract_path: vtable_config
                    .extract_path
                    .clone()
                    .unwrap_or_else(|| "results".to_string()),
            }
        } else if let Some(ref resource) = vtable_config.list_resource {
            let has_dynamic_params = vtable_config.uri_params.values().any(|v| v.is_dynamic());
            if has_dynamic_params {
                // Extract static params as plain strings for template defaults
                let default_params: HashMap<String, String> = vtable_config
                    .uri_params
                    .iter()
                    .filter_map(|(k, v)| v.as_static().map(|s| (k.clone(), s.to_string())))
                    .collect();

                // Build fallback queries for dynamic params with enumerate_from
                let prefix = entity_prefix.unwrap_or("");
                let fallbacks: HashMap<String, ResolvedFallback> = vtable_config
                    .uri_params
                    .iter()
                    .filter_map(|(k, v)| match v {
                        UriParamValue::Dynamic(d) => {
                            let table = format!("{}{}", prefix, d.enumerate_from.entity);
                            Some((
                                k.clone(),
                                ResolvedFallback {
                                    enumerate_sql: format!(
                                        "SELECT {} FROM {}",
                                        d.enumerate_from.field, table
                                    ),
                                },
                            ))
                        }
                        _ => None,
                    })
                    .collect();

                FetchMode::ResourceTemplate {
                    template: resource.clone(),
                    default_params,
                    fallbacks,
                }
            } else {
                let static_params: HashMap<String, String> = vtable_config
                    .uri_params
                    .iter()
                    .filter_map(|(k, v)| v.as_static().map(|s| (k.clone(), s.to_string())))
                    .collect();
                let uri = crate::mcp_sync_strategy::expand_uri_template(resource, &static_params)
                    .unwrap_or_else(|_| resource.clone());
                FetchMode::Resource { uri }
            }
        } else {
            panic!("VtableConfig must have either search_tool or list_resource");
        };

        Self {
            peer,
            fetch_mode,
            schema_sql,
            key_columns,
            column_to_param,
            column_names,
            id_scheme,
            cache_table,
            runtime,
        }
    }
}

impl ForeignDataWrapper for McpForeignDataWrapper {
    fn key_columns(&self) -> &[KeyColumn] {
        &self.key_columns
    }

    fn schema_sql(&self) -> String {
        self.schema_sql.clone()
    }

    fn open_cursor(&self, conn: Arc<CoreConnection>) -> Result<Box<dyn ForeignCursor>, LimboError> {
        let writeback = self.cache_table.as_ref().map(|table_name| WritebackTarget {
            conn: conn.clone(),
            cache_table: table_name.clone(),
            column_names: self.column_names.clone(),
        });

        Ok(Box::new(McpCursor {
            peer: self.peer.clone(),
            fetch_mode: self.fetch_mode.clone(),
            column_to_param: self.column_to_param.clone(),
            column_names: self.column_names.clone(),
            id_scheme: self.id_scheme.clone(),
            runtime: self.runtime.clone(),
            conn,
            writeback,
            rows: Vec::new(),
            index: 0,
            started: false,
        }))
    }
}

// ============================================================================
// McpCursor
// ============================================================================

/// Target for opportunistic cache writeback — writes fetched rows to a local
/// BTree table so IVM can track them.
struct WritebackTarget {
    conn: Arc<CoreConnection>,
    cache_table: String,
    column_names: Vec<String>,
}

impl WritebackTarget {
    /// Write rows to the cache table via INSERT OR REPLACE.
    fn write_rows(&self, rows: &[Vec<Value>]) -> Result<(), LimboError> {
        if rows.is_empty() {
            return Ok(());
        }

        let cols = self.column_names.join(", ");

        // Build a single INSERT OR REPLACE with multiple value tuples.
        // Turso's Connection::execute() doesn't support bind parameters,
        // so we inline the values as SQL literals.
        let value_rows: Vec<String> = rows
            .iter()
            .map(|row| {
                let vals: Vec<String> = row.iter().map(value_to_sql_literal).collect();
                format!("({})", vals.join(", "))
            })
            .collect();

        let sql = format!(
            "INSERT OR REPLACE INTO {} ({}) VALUES {}",
            self.cache_table,
            cols,
            value_rows.join(", ")
        );

        self.conn.execute(&sql).map_err(|e| {
            LimboError::ExtensionError(format!(
                "[WritebackTarget] Failed to write to '{}': {e}",
                self.cache_table
            ))
        })?;
        info!(
            "[WritebackTarget] Wrote {} rows to '{}'",
            rows.len(),
            self.cache_table
        );
        Ok(())
    }
}

struct McpCursor {
    peer: Arc<Peer<RoleClient>>,
    fetch_mode: FetchMode,
    column_to_param: HashMap<u32, String>,
    column_names: Vec<String>,
    id_scheme: Option<(String, String)>,
    runtime: tokio::runtime::Handle,
    /// Database connection for fallback enumeration queries.
    conn: Arc<CoreConnection>,
    writeback: Option<WritebackTarget>,
    rows: Vec<Vec<Value>>,
    index: usize,
    started: bool,
}

impl std::fmt::Debug for McpCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpCursor")
            .field("fetch_mode", &self.fetch_mode)
            .field("rows", &self.rows.len())
            .field("index", &self.index)
            .finish()
    }
}

// SAFETY: Peer<RoleClient> is Send+Sync, tokio Handle is Send+Sync.
unsafe impl Send for McpCursor {}
unsafe impl Sync for McpCursor {}

impl McpCursor {
    fn fetch_via_tool(
        &self,
        search_tool: &str,
        extract_path: &str,
        constraints: &[PushedConstraint],
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, LimboError> {
        let mut params: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for c in constraints {
            if let Some(param_name) = self.column_to_param.get(&c.column_index) {
                params.insert(param_name.clone(), turso_value_to_json(&c.value));
            }
        }

        info!(
            "[McpCursor] Calling tool '{}' with {} params",
            search_tool,
            params.len()
        );

        let peer = self.peer.clone();
        let tool_name = search_tool.to_string();

        let result = tokio::task::block_in_place(|| {
            self.runtime.block_on(async {
                peer.call_tool(CallToolRequestParam {
                    name: Cow::Owned(tool_name),
                    arguments: Some(params),
                })
                .await
            })
        })
        .map_err(|e| LimboError::ExtensionError(format!("MCP tool call failed: {e}")))?;

        if result.is_error == Some(true) {
            let error_text: String = result
                .content
                .iter()
                .filter_map(|c| c.as_text().map(|t| t.text.clone()))
                .collect::<Vec<_>>()
                .join("\n");
            return Err(LimboError::ExtensionError(format!(
                "MCP tool '{search_tool}' error: {error_text}"
            )));
        }

        let json_text: String = result
            .content
            .iter()
            .filter_map(|c| c.as_text().map(|t| t.text.clone()))
            .collect::<Vec<_>>()
            .join("");

        let response: serde_json::Value = serde_json::from_str(&json_text)
            .map_err(|e| LimboError::ExtensionError(format!("Failed to parse response: {e}")))?;

        let records = response
            .get(extract_path)
            .and_then(|v| v.as_array())
            .ok_or_else(|| {
                LimboError::ExtensionError(format!("Response missing '{extract_path}' array"))
            })?;

        Ok(records
            .iter()
            .filter_map(|r| r.as_object().cloned())
            .collect())
    }

    fn fetch_via_resource(
        &self,
        uri: &str,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, LimboError> {
        use rmcp::model::{ReadResourceRequestParam, ResourceContents};

        info!("[McpCursor] Reading resource '{}'", uri);

        let peer = self.peer.clone();
        let uri_owned = uri.to_string();

        let result = tokio::task::block_in_place(|| {
            self.runtime.block_on(async {
                peer.read_resource(ReadResourceRequestParam { uri: uri_owned })
                    .await
            })
        })
        .map_err(|e| LimboError::ExtensionError(format!("MCP read_resource failed: {e}")))?;

        let text: String = result
            .contents
            .into_iter()
            .filter_map(|c| match c {
                ResourceContents::TextResourceContents { text, .. } => Some(text),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");

        let parsed: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| LimboError::ExtensionError(format!("Failed to parse resource: {e}")))?;

        let records = parsed.as_array().ok_or_else(|| {
            LimboError::ExtensionError(format!("Resource '{uri}' did not return a JSON array"))
        })?;

        Ok(records
            .iter()
            .filter_map(|r| r.as_object().cloned())
            .collect())
    }
}

impl ForeignCursor for McpCursor {
    fn filter(&mut self, constraints: &[PushedConstraint]) -> Result<bool, LimboError> {
        let records = match &self.fetch_mode {
            FetchMode::Tool {
                search_tool,
                extract_path,
            } => self.fetch_via_tool(search_tool, extract_path, constraints)?,
            FetchMode::Resource { uri } => self.fetch_via_resource(uri)?,
            FetchMode::ResourceTemplate {
                template,
                default_params,
                fallbacks,
            } => {
                let mut params = default_params.clone();
                for c in constraints {
                    if let Some(param_name) = self.column_to_param.get(&c.column_index) {
                        if let Value::Text(ref t) = c.value {
                            params.insert(param_name.clone(), t.as_str().to_owned());
                        }
                    }
                }

                // For any unresolved params that have enumerate_from fallbacks,
                // query the referenced entity to get all values and fan out.
                let mut unresolved_with_fallback: Vec<(String, Vec<String>)> = Vec::new();
                for (param_name, fallback) in fallbacks {
                    if !params.contains_key(param_name) {
                        let values = enumerate_fallback_values(&self.conn, fallback)?;
                        if values.is_empty() {
                            return Ok(false);
                        }
                        unresolved_with_fallback.push((param_name.clone(), values));
                    }
                }

                if unresolved_with_fallback.is_empty() {
                    // All params resolved from WHERE — single fetch
                    let uri = crate::mcp_sync_strategy::expand_uri_template(template, &params)
                        .map_err(|e| {
                            LimboError::ExtensionError(format!("URI template param missing: {e}"))
                        })?;
                    self.fetch_via_resource(&uri)?
                } else {
                    // Fan out: enumerate all values and concatenate results.
                    // Currently supports a single fallback param (most common case).
                    assert_eq!(
                        unresolved_with_fallback.len(),
                        1,
                        "Multiple enumerate_from fallbacks not yet supported"
                    );
                    let (param_name, values) = &unresolved_with_fallback[0];
                    let mut all_records = Vec::new();
                    for value in values {
                        let mut p = params.clone();
                        p.insert(param_name.clone(), value.clone());
                        let uri = crate::mcp_sync_strategy::expand_uri_template(template, &p)
                            .map_err(|e| {
                                LimboError::ExtensionError(format!(
                                    "URI template param missing: {e}"
                                ))
                            })?;
                        let records = self.fetch_via_resource(&uri).map_err(|e| {
                            LimboError::ExtensionError(format!(
                                "[McpCursor] fetch_via_resource failed for {} = {}: {e}",
                                param_name, value
                            ))
                        })?;
                        all_records.extend(records);
                    }
                    all_records
                }
            }
        };

        // Convert JSON records to rows of Turso Values, aligned with schema column order.
        // Each record is a JSON object — we extract values by column name in schema order.
        // Missing fields become NULL, extra fields are ignored.
        self.rows = records
            .iter()
            .map(|obj| {
                self.column_names
                    .iter()
                    .map(|col_name| {
                        let val = obj
                            .get(col_name)
                            .map(json_value_to_turso_value)
                            .unwrap_or(Value::Null);
                        // Apply ID scheme prefix if this is the ID column
                        if let Some((ref id_col, ref scheme)) = self.id_scheme {
                            if col_name == id_col {
                                if let Value::Text(ref t) = val {
                                    return Value::build_text(format!("{scheme}:{}", t.as_str()));
                                }
                            }
                        }
                        val
                    })
                    .collect()
            })
            .collect();

        self.index = 0;
        self.started = true;

        info!(
            "[McpCursor] Got {} records via {:?}",
            self.rows.len(),
            self.fetch_mode
        );

        if let Some(ref wb) = self.writeback {
            wb.write_rows(&self.rows)?;
        }

        Ok(!self.rows.is_empty())
    }

    fn next(&mut self) -> Result<bool, LimboError> {
        if !self.started {
            return Ok(false);
        }
        self.index += 1;
        Ok(self.index < self.rows.len())
    }

    fn column(&self, idx: usize) -> Result<Value, LimboError> {
        let row = &self.rows[self.index];
        if idx < row.len() {
            Ok(row[idx].clone())
        } else {
            Ok(Value::Null)
        }
    }

    fn rowid(&self) -> i64 {
        self.index as i64
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Run a fallback enumeration query (e.g. `SELECT id FROM cc_session`) and collect
/// all text values from the first column.
fn enumerate_fallback_values(
    conn: &Arc<CoreConnection>,
    fallback: &ResolvedFallback,
) -> Result<Vec<String>, LimboError> {
    info!(
        "[McpCursor] Enumerating fallback values: {}",
        fallback.enumerate_sql
    );
    let mut stmt = conn.query(&fallback.enumerate_sql)?.ok_or_else(|| {
        LimboError::ExtensionError(format!(
            "Fallback query returned no statement: {}",
            fallback.enumerate_sql
        ))
    })?;

    let mut values = Vec::new();
    loop {
        match stmt.step()? {
            turso_core::StepResult::Row => {
                if let Some(row) = stmt.row() {
                    match row.get_value(0) {
                        Value::Text(t) => values.push(t.as_str().to_owned()),
                        Value::Numeric(turso_core::Numeric::Integer(i)) => {
                            values.push(i.to_string())
                        }
                        _ => {}
                    }
                }
            }
            turso_core::StepResult::Done => break,
            turso_core::StepResult::IO => continue,
            _ => break,
        }
    }

    info!(
        "[McpCursor] Fallback enumeration got {} values",
        values.len()
    );
    Ok(values)
}

/// Convert a Turso Value to a SQL literal string for INSERT statements.
fn value_to_sql_literal(v: &Value) -> String {
    use turso_core::Numeric;
    match v {
        Value::Null => "NULL".to_string(),
        Value::Numeric(Numeric::Integer(i)) => i.to_string(),
        Value::Numeric(Numeric::Float(f)) => format!("{}", **f),
        Value::Text(t) => {
            // Escape single quotes by doubling them
            let escaped = t.as_str().replace('\'', "''");
            format!("'{escaped}'")
        }
        Value::Blob(b) => format!("X'{}'", hex_encode(b)),
    }
}

/// Convert a Turso Value to a serde_json::Value for MCP tool params.
fn turso_value_to_json(v: &Value) -> serde_json::Value {
    use turso_core::Numeric;
    match v {
        Value::Null => serde_json::Value::Null,
        Value::Numeric(Numeric::Integer(i)) => serde_json::Value::Number((*i).into()),
        Value::Numeric(Numeric::Float(f)) => serde_json::Number::from_f64(**f)
            .map(serde_json::Value::Number)
            .unwrap_or(serde_json::Value::Null),
        Value::Text(t) => serde_json::Value::String(t.as_str().to_owned()),
        Value::Blob(b) => serde_json::Value::String(hex_encode(b)),
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        write!(s, "{:02x}", b).unwrap();
    }
    s
}

/// Convert a serde_json::Value to a Turso Value (for record rows).
fn json_value_to_turso_value(v: &serde_json::Value) -> Value {
    match v {
        serde_json::Value::Null => Value::Null,
        serde_json::Value::Bool(b) => Value::from_i64(if *b { 1 } else { 0 }),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::from_i64(i)
            } else if let Some(f) = n.as_f64() {
                Value::from_f64(f)
            } else {
                Value::build_text(n.to_string())
            }
        }
        serde_json::Value::String(s) => Value::build_text(s.clone()),
        serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            Value::build_text(v.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_constraint_ops() {
        assert_eq!(parse_constraint_op("eq"), Some(ConstraintOp::Eq));
        assert_eq!(parse_constraint_op("="), Some(ConstraintOp::Eq));
        assert_eq!(parse_constraint_op("gt"), Some(ConstraintOp::Gt));
        assert_eq!(parse_constraint_op(">="), Some(ConstraintOp::Ge));
        assert_eq!(parse_constraint_op("like"), Some(ConstraintOp::Like));
        assert_eq!(parse_constraint_op("unknown"), None);
    }

    #[test]
    fn vtable_config_deserialize() {
        let yaml = r#"
search_tool: search-emails
extract_path: emails
get_tool: get-email
write_through: true
filter_mapping:
  from_address:
    param: from
    ops: [eq, like]
  date:
    param: after
    ops: [gt, ge]
    required: true
"#;
        let config: VtableConfig = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(config.search_tool.as_deref(), Some("search-emails"));
        assert_eq!(config.extract_path.as_deref(), Some("emails"));
        assert!(config.list_resource.is_none());
        assert!(config.write_through);
        assert_eq!(config.filter_mapping.len(), 2);

        let from = &config.filter_mapping["from_address"];
        assert_eq!(from.param, "from");
        assert_eq!(from.ops, vec!["eq", "like"]);
        assert!(!from.required);

        let date = &config.filter_mapping["date"];
        assert_eq!(date.param, "after");
        assert!(date.required);
    }

    #[test]
    fn build_key_columns_from_config() {
        let yaml = r#"
search_tool: search-emails
extract_path: emails
filter_mapping:
  subject:
    param: query
    ops: [eq, like]
  from_address:
    param: from
    ops: [eq]
    required: true
"#;
        let config: VtableConfig = serde_yaml::from_str(yaml).unwrap();
        let columns = vec![
            ("msg_id".to_string(), "TEXT".to_string()),
            ("subject".to_string(), "TEXT".to_string()),
            ("from_address".to_string(), "TEXT".to_string()),
        ];

        let (key_columns, column_to_param, schema_sql, column_names) =
            build_fdw_metadata("test_email", &columns, &config);

        assert_eq!(key_columns.len(), 2);

        // subject at column index 1
        let subj_kc = key_columns.iter().find(|kc| kc.name == "subject").unwrap();
        assert_eq!(subj_kc.column_index, 1);
        assert!(!subj_kc.required);
        assert_eq!(subj_kc.operators.len(), 2);

        // from_address at column index 2, required
        let from_kc = key_columns
            .iter()
            .find(|kc| kc.name == "from_address")
            .unwrap();
        assert_eq!(from_kc.column_index, 2);
        assert!(from_kc.required);

        // column_to_param mapping
        assert_eq!(column_to_param.get(&1), Some(&"query".to_string()));
        assert_eq!(column_to_param.get(&2), Some(&"from".to_string()));

        assert!(schema_sql.contains("test_email"));
        assert!(schema_sql.contains("msg_id TEXT"));

        // column_names preserves schema order
        assert_eq!(column_names, vec!["msg_id", "subject", "from_address"]);
    }

    #[test]
    fn dynamic_uri_params_register_key_columns() {
        let yaml = r#"
list_resource: "claude-history://sessions/{session_id}/messages"
uri_params:
  session_id: ""
"#;
        let config: VtableConfig = serde_yaml::from_str(yaml).unwrap();
        let columns = vec![
            ("id".to_string(), "TEXT".to_string()),
            ("session_id".to_string(), "TEXT".to_string()),
            ("content".to_string(), "TEXT".to_string()),
        ];

        let (key_columns, column_to_param, _schema_sql, _column_names) =
            build_fdw_metadata("cc_message", &columns, &config);

        // session_id should be auto-registered as a required key column
        assert_eq!(key_columns.len(), 1);
        let kc = &key_columns[0];
        assert_eq!(kc.name, "session_id");
        assert_eq!(kc.column_index, 1);
        assert!(kc.required);
        assert_eq!(kc.operators, vec![ConstraintOp::Eq]);

        // column_to_param maps column index → param name (same as column name)
        assert_eq!(column_to_param.get(&1), Some(&"session_id".to_string()));
    }

    #[test]
    fn resource_template_fetch_mode_from_config() {
        // Empty string → dynamic (required from WHERE)
        let yaml = r#"
list_resource: "claude-history://sessions/{session_id}/messages"
uri_params:
  session_id: ""
"#;
        let config: VtableConfig = serde_yaml::from_str(yaml).unwrap();
        let has_dynamic = config.uri_params.values().any(|v| v.is_dynamic());
        assert!(has_dynamic);

        // Non-empty string → static (baked in)
        let yaml_static = r#"
list_resource: "claude-history://sessions/{session_id}/messages"
uri_params:
  session_id: "abc-123"
"#;
        let config_static: VtableConfig = serde_yaml::from_str(yaml_static).unwrap();
        let has_dynamic_static = config_static.uri_params.values().any(|v| v.is_dynamic());
        assert!(!has_dynamic_static);

        // Structured enumerate_from → dynamic (with fallback)
        let yaml_fallback = r#"
list_resource: "claude-history://sessions/{session_id}/messages"
uri_params:
  session_id:
    enumerate_from:
      entity: session
      field: id
"#;
        let config_fb: VtableConfig = serde_yaml::from_str(yaml_fallback).unwrap();
        let has_dynamic_fb = config_fb.uri_params.values().any(|v| v.is_dynamic());
        assert!(has_dynamic_fb);
        // Should parse as Dynamic variant
        assert!(matches!(
            config_fb.uri_params.get("session_id"),
            Some(UriParamValue::Dynamic(_))
        ));
    }

    #[test]
    fn enumerate_from_not_required_in_key_columns() {
        let yaml = r#"
list_resource: "claude-history://sessions/{session_id}/messages"
uri_params:
  session_id:
    enumerate_from:
      entity: session
      field: id
"#;
        let config: VtableConfig = serde_yaml::from_str(yaml).unwrap();
        let columns = vec![
            ("id".to_string(), "TEXT".to_string()),
            ("session_id".to_string(), "TEXT".to_string()),
            ("content".to_string(), "TEXT".to_string()),
        ];

        let (key_columns, column_to_param, _schema_sql, _column_names) =
            build_fdw_metadata("cc_message", &columns, &config);

        // session_id should be registered but NOT required (has fallback)
        assert_eq!(key_columns.len(), 1);
        let kc = &key_columns[0];
        assert_eq!(kc.name, "session_id");
        assert!(!kc.required);
        assert_eq!(column_to_param.get(&1), Some(&"session_id".to_string()));
    }

    #[test]
    fn turso_value_json_roundtrip() {
        let v = Value::build_text("hello");
        let j = turso_value_to_json(&v);
        assert_eq!(j, serde_json::Value::String("hello".to_string()));

        let v = Value::from_i64(42);
        let j = turso_value_to_json(&v);
        assert_eq!(j, serde_json::json!(42));

        let j = serde_json::json!("world");
        let v = json_value_to_turso_value(&j);
        assert_eq!(v, Value::build_text("world"));
    }
}

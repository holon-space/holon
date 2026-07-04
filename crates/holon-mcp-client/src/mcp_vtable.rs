//! MCP Foreign Data Wrapper — queries external MCP servers through Turso's FDW
//! API.
//!
//! Translates SQL WHERE constraints into MCP tool parameters via a declarative
//! `FilterMapping`, then fetches results through `peer.call_tool()`.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc;

use futures::stream::StreamExt;
use futures::stream::TryStreamExt;
use rmcp::model::CallToolRequestParam;
use serde::Deserialize;
use serde::Serialize;
use tracing::debug;
use tracing::info;
use tracing::warn;
use turso_core::Connection as CoreConnection;
use turso_core::LimboError;
use turso_core::Value;
use turso_core::foreign::FdwChange;
use turso_core::foreign::ForeignCursor;
use turso_core::foreign::ForeignDataWrapper;
use turso_core::foreign::KeyColumn;
use turso_core::foreign::PushedConstraint;
use turso_core::foreign::StreamingForeignData;
use turso_ext::ConstraintOp;

use crate::mcp_call_surface::McpCallSurface;

// ============================================================================
// YAML sidecar config types
// ============================================================================

/// What a source honestly promises about a fetch — the property that decides
/// which maintenance mechanism is SOUND for an entity, declared rather than
/// inferred (RULED 2026-08-06).
///
/// The one thing a fetch response cannot tell you is what it did NOT return,
/// and every mechanism below differs only in how much absence it may read as
/// deletion. Guessing has exactly one failure mode, and it is silent mass data
/// loss, so the author states it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FetchContract {
    /// Every fetch returns the ENTIRE collection. Absence is deletion, so a
    /// full REFRESH is sound and its sweep retracts what vanished.
    Snapshot,
    /// Every fetch returns the entire collection WITHIN a scope the fetch
    /// itself chose (here: the `enumerate_from` watermark). Absence carries no
    /// information — an unreturned row may be deleted or merely out of scope —
    /// so the live path may only UPSERT, and deletions are reconciled by a
    /// (scoped) REFRESH.
    ScopedSnapshot,
    /// The provider sends upserts AND tombstones, so absence never has to be
    /// interpreted at all. No claude-history entity offers this today.
    Delta,
}

/// Virtual table configuration for an entity in the YAML sidecar.
///
/// Supports two fetch modes:
/// - **Tool-based**: `search_tool` + `extract_path` — calls an MCP tool with
///   filter pushdown
/// - **Resource-based**: `list_resource` — reads an MCP resource URI (no
///   pushdown, full fetch)
///
/// Exactly one of `search_tool` or `list_resource` must be set.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct VtableConfig {
    /// What this entity's source promises about a fetch. Required before the
    /// table may be subscribed to for streaming push: without it the driver
    /// would have to guess how much absence means, which is the one guess with
    /// a silent-data-loss failure mode.
    #[serde(default)]
    pub fetch_contract: Option<FetchContract>,
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
    /// If true, write fetched results back to the cache table (opportunistic
    /// caching).
    #[serde(default)]
    pub write_through: bool,
    /// Maps column names to MCP tool parameters with supported operators.
    /// Only meaningful for tool-based mode.
    #[serde(default)]
    pub filter_mapping: HashMap<String, FilterColumnConfig>,
    /// Constant arguments injected into every tool call.
    ///
    /// Merged into the params dict alongside WHERE-derived and enumeration-
    /// derived values. Useful for `minimal_output: false`, fixed `state`
    /// filters, etc. WHERE constraints win over static args on key collision.
    #[serde(default)]
    pub static_args: serde_json::Map<String, serde_json::Value>,
    /// Per-column extraction overrides. The map key is the SQL column name;
    /// the value declares how to read it out of each response record.
    /// Columns without an entry use a flat `obj[column_name]` lookup.
    #[serde(default)]
    pub columns: HashMap<String, ColumnConfig>,
    /// Pagination strategy. When unset, only one call is made per fetch.
    #[serde(default)]
    pub pagination: Option<PaginationConfig>,
    /// Hard bound on `enumerate_from` fan-out. When the enumeration produces
    /// more parent rows than this, the query FAILS LOUD (no silent
    /// truncation) naming the limit and the actual count. `None` = unbounded.
    #[serde(default)]
    pub max_fan_out: Option<u64>,
}

/// Pagination strategy declared by the YAML.
///
/// Three concrete styles cover the GitHub MCP surface; new MCP integrations
/// pick the closest match or add a variant.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(tag = "style", rename_all = "snake_case")]
pub enum PaginationConfig {
    /// Cursor-based pagination. After each call, read the next cursor from
    /// `cursor_response_path` and inject it as `cursor_param`. Continue while
    /// `has_more_path` resolves to truthy.
    Cursor {
        /// Dotted path to next cursor in response (e.g. `pageInfo.endCursor`).
        cursor_response_path: String,
        /// Dotted path to boolean "has more pages" (e.g.
        /// `pageInfo.hasNextPage`).
        has_more_path: String,
        /// Param name to send cursor back as (e.g. `after`).
        cursor_param: String,
        /// Optional `perPage` param name to send with the page size.
        #[serde(default)]
        size_param: Option<String>,
        /// Page size to request when `size_param` is set.
        #[serde(default)]
        page_size: Option<u32>,
    },
    /// Page-number with total. Loop while accumulated rows < total.
    PageTotal {
        /// `page` param name (1-based).
        page_param: String,
        /// `perPage` param name.
        size_param: String,
        /// Page size to request.
        page_size: u32,
        /// Dotted path to total count in response (e.g. `total_count`).
        total_response_path: String,
    },
    /// Page-number, no total. Stop when a page returns fewer than `page_size`.
    PageShort {
        page_param: String,
        size_param: String,
        page_size: u32,
    },
}

/// How to extract one SQL column from a JSON response record.
#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct ColumnConfig {
    /// Dotted JSON path into each record (e.g. `owner.login`). Defaults to the
    /// column name when unset.
    #[serde(default)]
    pub source_path: Option<String>,
    /// Encoding for non-scalar values landing in TEXT columns.
    ///
    /// - `None` — default, copy the JSON value as-is via
    ///   `json_value_to_turso_value`.
    /// - `Some("json")` — `serde_json::to_string` arrays/objects so they fit a
    ///   TEXT cell.
    #[serde(default)]
    pub encoding: Option<String>,
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
    /// Enumeration enumeration when the value isn't supplied by WHERE.
    /// Carries either a single `field` (legacy: binds to this column's own
    /// `param`) or paired `fields` (new: param_name → parent_column for FK
    /// fan-out across multiple correlated params).
    #[serde(default)]
    pub enumerate_from: Option<EnumerateFrom>,
}

fn default_ops() -> Vec<String> {
    vec!["eq".to_string()]
}

/// A URI template parameter value — either static, dynamic (required from
/// WHERE), or dynamic with a enumeration enumeration from another entity.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum UriParamValue {
    /// Structured: dynamic param with enumeration enumeration.
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

    /// Whether this param must be resolved dynamically (from WHERE or
    /// enumeration).
    pub fn is_dynamic(&self) -> bool {
        self.as_static().is_none()
    }
}

/// Dynamic URI param with a enumeration: when WHERE doesn't provide the value,
/// enumerate all values from the referenced entity's field.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct DynamicUriParam {
    pub enumerate_from: EnumerateFrom,
}

/// Reference to another entity for FK enumeration.
///
/// Two shapes (one of `field` or `fields` must be set):
/// - **Legacy single-field**: `field: id` — enumerates one value per parent
///   row, bound to the owning column's `param`.
/// - **Paired multi-field**: `fields: { tool_param: parent_column, ... }` —
///   enumerates a correlated tuple per parent row. Used to bind multiple FK
///   columns (e.g. `owner` + `repo`) from a single parent row so fan-out never
///   produces mismatched param combinations.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EnumerateFrom {
    /// Entity name (without prefix), e.g. `"session"`, `"repository"`.
    pub entity: String,
    /// Legacy single-field — binds to the owning column's `param`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Paired multi-field — `tool_param_name → parent_column_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<HashMap<String, String>>,
    /// Optional SQL predicate appended as `WHERE {where}` to the enumeration
    /// query. Runs as pure local SQL against the parent cache table, so
    /// correlated-subquery watermarks (incremental refresh) are expressible.
    /// YAML key: `where`.
    #[serde(default, rename = "where", skip_serializing_if = "Option::is_none")]
    pub where_sql: Option<String>,
    /// Optional `ORDER BY` clause body for the enumeration query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order_by: Option<String>,
    /// Optional `LIMIT` for the enumeration query.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u64>,
}

impl EnumerateFrom {
    /// Bindings as `(parent_column, tool_param)` pairs. `owning_param` is the
    /// param name owning this `EnumerateFrom` (used only for the legacy
    /// single-field shape).
    fn raw_bindings(&self, owning_param: &str) -> Vec<(String, String)> {
        match (&self.fields, &self.field) {
            (Some(map), _) => map
                .iter()
                .map(|(param, col)| (col.clone(), param.clone()))
                .collect(),
            (None, Some(f)) => vec![(f.clone(), owning_param.to_string())],
            (None, None) => panic!(
                "EnumerateFrom for entity '{}' must set either `field` or `fields`",
                self.entity
            ),
        }
    }
}

/// One enumerated parent column bound to one tool/URI param.
#[derive(Debug, Clone)]
struct EnumerationBinding {
    parent_col: String,
    tool_param: String,
    /// `Some(scheme)` when `parent_col` is the parent entity's id column:
    /// cached ids are stored scheme-prefixed (`{scheme}:{raw}`, matching the
    /// McpSyncEngine / id_scheme convention) but the MCP server expects the
    /// raw id. The value is unprefixed at the enumeration boundary
    /// ([`run_enumeration`]) and it is a LOUD error for a cached value to
    /// lack the expected prefix.
    strip_scheme: Option<String>,
}

/// Resolved enumeration source — pre-computed SQL + binding map.
#[derive(Debug, Clone)]
struct ResolvedEnumeration {
    /// SQL selecting the parent columns referenced by `bindings`, in order.
    /// e.g. `SELECT id FROM cc_session`, `SELECT owner, name FROM
    /// gh_repository`.
    enumerate_sql: String,
    /// Bindings aligned with the SQL's SELECT order.
    bindings: Vec<EnumerationBinding>,
}

/// URI scheme used to prefix cached ids of `{prefix}{entity}` — same
/// normalization as `holon_api::EntityName` (underscores → hyphens).
fn id_scheme_for_entity(prefix: &str, entity: &str) -> String {
    format!("{prefix}{entity}").replace('_', "-")
}

impl ResolvedEnumeration {
    fn from_enumerate_from(
        ef: &EnumerateFrom,
        owning_param: &str,
        prefix: &str,
        parent_id_column: &str,
    ) -> Self {
        let bindings: Vec<EnumerationBinding> = ef
            .raw_bindings(owning_param)
            .into_iter()
            .map(|(parent_col, tool_param)| {
                let strip_scheme = (parent_col == parent_id_column)
                    .then(|| id_scheme_for_entity(prefix, &ef.entity));
                EnumerationBinding {
                    parent_col,
                    tool_param,
                    strip_scheme,
                }
            })
            .collect();
        let cols: Vec<&str> = bindings.iter().map(|b| b.parent_col.as_str()).collect();
        let table = format!("{}{}", prefix, ef.entity);
        let mut enumerate_sql = format!("SELECT {} FROM {}", cols.join(", "), table);
        if let Some(w) = &ef.where_sql {
            enumerate_sql.push_str(&format!(" WHERE {w}"));
        }
        if let Some(o) = &ef.order_by {
            enumerate_sql.push_str(&format!(" ORDER BY {o}"));
        }
        if let Some(n) = ef.limit {
            enumerate_sql.push_str(&format!(" LIMIT {n}"));
        }
        Self {
            enumerate_sql,
            bindings,
        }
    }
}

/// How the FDW fetches data from the MCP server.
#[derive(Debug, Clone)]
enum FetchMode {
    /// Call an MCP tool with optional filter pushdown.
    Tool {
        search_tool: String,
        /// JSON key containing the records array. When `None`, the response is
        /// expected to be a bare top-level array.
        extract_path: Option<String>,
        /// Constant args merged into every call (e.g. `minimal_output: false`).
        static_args: serde_json::Map<String, serde_json::Value>,
        /// Pagination loop strategy. `None` ⇒ a single call.
        pagination: Option<PaginationConfig>,
        /// Owning-param-name → enumeration source for FK fan-out. When the
        /// owning param isn't supplied by WHERE, run the enumeration SQL and
        /// fan out one tool call per parent row. Each enumeration may carry
        /// multiple correlated bindings (e.g. `owner`+`repo` together).
        enumerations: HashMap<String, ResolvedEnumeration>,
    },
    /// Read an MCP resource URI (returns JSON array, no pushdown).
    Resource { uri: String },
    /// Read an MCP resource URI with dynamic template params resolved from
    /// WHERE constraints.
    ResourceTemplate {
        template: String,
        /// Static params baked in from config (non-empty values).
        default_params: HashMap<String, String>,
        /// Owning-param-name → enumeration source. Same semantics as
        /// `FetchMode::Tool::enumerations` but applied to URI template params.
        enumerations: HashMap<String, ResolvedEnumeration>,
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

/// One parent's fan-out result: its index in the parent batch, the key
/// bindings used, and the child rows fetched for it.
type FetchedChildRows = (
    usize,
    HashMap<String, String>,
    Vec<serde_json::Map<String, serde_json::Value>>,
);

/// Live subscription state shared between the wrapper and every cursor it
/// opens. The push path is stateless beyond this: it holds no snapshot of the
/// mirror, because it never needs to know what the mirror used to contain.
#[derive(Debug, Default)]
struct StreamState {
    sender: Option<mpsc::Sender<FdwChange>>,
    /// Constraints the subscription was opened with — replayed on every
    /// re-fetch so the pushed rows stay predicate-scoped like the mirror.
    constraints: Vec<PushedConstraint>,
}

/// Canonical, total, injective-per-type rendering of a row's identity values.
/// `Value` is neither `Hash` nor `Eq`, so within-scan duplicate detection keys
/// on this instead; the type tag and the length prefix keep `Text("1")` and
/// `Integer(1)`, and `["a", "b"]` and `["a|b"]`, distinct.
fn identity_key(row: &[Value], identity_columns: &[u32]) -> String {
    use turso_core::Numeric;
    let mut key = String::new();
    for idx in identity_columns {
        let v = &row[*idx as usize];
        match v {
            Value::Null => key.push_str("N;"),
            Value::Numeric(Numeric::Integer(i)) => key.push_str(&format!("I{i};")),
            Value::Numeric(Numeric::Float(f)) => key.push_str(&format!("F{};", f.to_bits())),
            Value::Text(t) => {
                let s = t.as_str();
                key.push_str(&format!("S{}:{s};", s.len()));
            }
            Value::Blob(b) => key.push_str(&format!("B{}:{};", b.len(), hex_encode(b))),
        }
    }
    key
}

/// A [`ForeignDataWrapper`] that queries an MCP server via tool calls.
///
/// Constructed from the YAML sidecar `vtable:` config and the live MCP peer.
/// Registered at startup via `conn.register_foreign_table()`.
#[derive(Debug)]
pub struct McpForeignDataWrapper {
    /// Live connection to the MCP server.
    peer: Arc<dyn McpCallSurface>,
    /// SQL table name — the handle the engine's push path is keyed by.
    table_name: String,
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
    /// Row identity for incremental matview maintenance — the indices of the
    /// declared identity columns. `None` is the engine-sanctioned opt-out: the
    /// entity declared no identity, so matviews over it keep snapshot
    /// semantics and no mirror is built.
    identity_columns: Option<Vec<u32>>,
    /// What the source promises about a fetch (see [`FetchContract`]).
    /// `None` = undeclared, which forbids streaming push.
    fetch_contract: Option<FetchContract>,
    /// Subscription shared with every cursor this wrapper opens.
    stream: Arc<Mutex<StreamState>>,
    /// If set, fetched rows are written to this cache table via INSERT OR
    /// REPLACE.
    cache_table: Option<String>,
    /// Per-column extraction config (dotted source_path, encoding).
    column_configs: HashMap<String, ColumnConfig>,
    /// Hard bound on enumeration fan-out (see [`VtableConfig::max_fan_out`]).
    max_fan_out: Option<u64>,
    /// Tokio runtime handle for async→sync bridge in filter().
    runtime: tokio::runtime::Handle,
}

/// Build key columns, column→param mapping, and schema DDL from sidecar config.
/// Extracted for testability (doesn't require a live MCP Peer).
///
/// Key columns come from two sources:
/// 1. Explicit `filter_mapping` entries (tool-based pushdown)
/// 2. Empty `uri_params` values (resource template pushdown — column name must
///    match param name)
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

    // Pre-compute the set of tool params that get bound by some enumeration
    // (either as the owning column or as a paired binding). These are
    // non-required at the KeyColumn level regardless of YAML `required:`.
    let mut enumeration_bound_params: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    for fc in vtable_config.filter_mapping.values() {
        if let Some(ef) = &fc.enumerate_from {
            enumeration_bound_params.insert(fc.param.clone());
            for (_parent_col, tool_param) in ef.raw_bindings(&fc.param) {
                enumeration_bound_params.insert(tool_param);
            }
        }
    }

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

            let is_enumeration_bound = enumeration_bound_params.contains(&filter_config.param);
            let mut kc = KeyColumn::new(col_name.clone(), col_idx as u32, ops);
            if filter_config.required && !is_enumeration_bound {
                kc = kc.required();
            }
            column_to_param.insert(col_idx as u32, filter_config.param.clone());
            key_columns.push(kc);
        }
        // Source 2: dynamic URI template params (column name == param name, Eq-only)
        // Required only if there's no enumerate_from enumeration.
        else if let Some(param_val) = vtable_config.uri_params.get(col_name)
            && param_val.is_dynamic()
        {
            let has_enumeration = matches!(param_val, UriParamValue::Dynamic(_));
            let mut kc = KeyColumn::new(col_name.clone(), col_idx as u32, vec![ConstraintOp::Eq]);
            if !has_enumeration {
                kc = kc.required();
            }
            column_to_param.insert(col_idx as u32, col_name.clone());
            key_columns.push(kc);
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
    /// `identity_columns` names the schema columns forming the row identity
    /// (see `EntityConfig::identity_columns`); empty means the entity declares
    /// none, which downgrades the table to snapshot semantics with a warning.
    /// It is NOT `id_scheme`'s column: scheme-prefixing is a value convention,
    /// identity is a declaration, and the two coincide only by accident.
    /// `cache_table` is the name of the local BTree table to write through to.
    /// `entity_prefix` is needed to resolve `enumerate_from` entity references
    /// to actual SQL table names (e.g. `"cc_"` + `"session"` → `"cc_session"`).
    /// `enumeration_id_columns` maps raw entity names to their id column
    /// (entities absent from the map default to `"id"`); it drives the
    /// scheme-strip boundary for enumerated parent ids.
    #[allow(clippy::too_many_arguments)] // constructor wires the vtable's full surface
    pub fn new(
        table_name: &str,
        columns: &[(String, String)],
        vtable_config: &VtableConfig,
        peer: Arc<dyn McpCallSurface>,
        id_scheme: Option<(String, String)>,
        identity_columns: &[String],
        cache_table: Option<String>,
        runtime: tokio::runtime::Handle,
        entity_prefix: Option<&str>,
        enumeration_id_columns: &HashMap<String, String>,
    ) -> Self {
        let (key_columns, column_to_param, schema_sql, column_names) =
            build_fdw_metadata(table_name, columns, vtable_config);

        let prefix = entity_prefix.unwrap_or("");
        let id_col_of = |entity: &str| -> String {
            enumeration_id_columns
                .get(entity)
                .cloned()
                .unwrap_or_else(|| "id".to_string())
        };
        let fetch_mode = if let Some(ref tool) = vtable_config.search_tool {
            // Tool-mode FK enumerations: one entry per filter_mapping column
            // that declares `enumerate_from`. The map is keyed by the owning
            // tool-param name (matches the params dict built in fetch_via_tool).
            let enumerations: HashMap<String, ResolvedEnumeration> = vtable_config
                .filter_mapping
                .values()
                .filter_map(|fc| {
                    fc.enumerate_from.as_ref().map(|ef| {
                        (
                            fc.param.clone(),
                            ResolvedEnumeration::from_enumerate_from(
                                ef,
                                &fc.param,
                                prefix,
                                &id_col_of(&ef.entity),
                            ),
                        )
                    })
                })
                .collect();

            FetchMode::Tool {
                search_tool: tool.clone(),
                extract_path: vtable_config.extract_path.clone(),
                static_args: vtable_config.static_args.clone(),
                pagination: vtable_config.pagination.clone(),
                enumerations,
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

                // Build enumerations for dynamic params with enumerate_from.
                // URI mode currently only uses the legacy single-field shape.
                let enumerations: HashMap<String, ResolvedEnumeration> = vtable_config
                    .uri_params
                    .iter()
                    .filter_map(|(k, v)| match v {
                        UriParamValue::Dynamic(d) => Some((
                            k.clone(),
                            ResolvedEnumeration::from_enumerate_from(
                                &d.enumerate_from,
                                k,
                                prefix,
                                &id_col_of(&d.enumerate_from.entity),
                            ),
                        )),
                        _ => None,
                    })
                    .collect();

                FetchMode::ResourceTemplate {
                    template: resource.clone(),
                    default_params,
                    enumerations,
                }
            } else {
                let static_params: HashMap<String, String> = vtable_config
                    .uri_params
                    .iter()
                    .filter_map(|(k, v)| v.as_static().map(|s| (k.clone(), s.to_string())))
                    .collect();
                let uri = crate::mcp_sync_strategy::expand_uri_template(resource, &static_params)
                    .unwrap_or_else(|e| {
                        panic!(
                            "list_resource template '{resource}' failed to expand with static \
                             uri_params {static_params:?}: {e}"
                        )
                    });
                FetchMode::Resource { uri }
            }
        } else {
            panic!("VtableConfig must have either search_tool or list_resource");
        };

        let identity_columns = if identity_columns.is_empty() {
            warn!(
                "[McpForeignDataWrapper] table '{table_name}' declares no row identity (no \
                 primary_key column, no id_column, no 'id' column among {column_names:?}) — it \
                 falls back to snapshot semantics and CANNOT be maintained incrementally: \
                 matviews over it only update on REFRESH"
            );
            None
        } else {
            Some(
                identity_columns
                    .iter()
                    .map(|id_col| {
                        let idx = column_names
                            .iter()
                            .position(|c| c == id_col)
                            .unwrap_or_else(|| {
                                panic!(
                                    "[McpForeignDataWrapper] table '{table_name}' declares id \
                                     column '{id_col}' which is not among its schema columns \
                                     {column_names:?}; a row identity that names no column cannot \
                                     be maintained"
                                )
                            });
                        idx as u32
                    })
                    .collect(),
            )
        };

        Self {
            peer,
            table_name: table_name.to_string(),
            fetch_mode,
            schema_sql,
            key_columns,
            column_to_param,
            column_names,
            id_scheme,
            identity_columns,
            fetch_contract: vtable_config.fetch_contract,
            stream: Arc::new(Mutex::new(StreamState::default())),
            cache_table,
            column_configs: vtable_config.columns.clone(),
            max_fan_out: vtable_config.max_fan_out,
            runtime,
        }
    }
}

impl ForeignDataWrapper for McpForeignDataWrapper {
    fn key_columns(&self) -> &[KeyColumn] {
        &self.key_columns
    }

    fn identity_columns(&self) -> Option<&[u32]> {
        self.identity_columns.as_deref()
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
            column_configs: self.column_configs.clone(),
            id_scheme: self.id_scheme.clone(),
            max_fan_out: self.max_fan_out,
            runtime: self.runtime.clone(),
            conn,
            writeback,
            fan_out_groups: Vec::new(),
            rows: Vec::new(),
            index: 0,
            started: false,
        }))
    }
}

impl McpForeignDataWrapper {
    /// Subscribe to row-level changes. Inherent so callers holding the concrete
    /// wrapper need no trait import; [`StreamingForeignData`] delegates here.
    pub fn subscribe(
        &self,
        constraints: &[PushedConstraint],
    ) -> Result<mpsc::Receiver<FdwChange>, LimboError> {
        if self.identity_columns.is_none() {
            return Err(LimboError::ExtensionError(format!(
                "[McpForeignDataWrapper] table '{}' declares no row identity, so a subscription \
                 to it cannot be maintained incrementally: a change could never be matched to \
                 the row it replaces. Mark the entity's key column primary_key, or use REFRESH.",
                self.table_name
            )));
        }
        if self.fetch_contract.is_none() {
            return Err(LimboError::ExtensionError(format!(
                "[McpForeignDataWrapper] table '{}' declares no `fetch_contract`, so what its \
                 source promises about a fetch is unknown — and that is precisely what decides \
                 whether a row missing from a re-fetch was deleted or merely out of scope. \
                 Declare `vtable.fetch_contract` (snapshot | scoped_snapshot | delta) on the \
                 entity.",
                self.table_name
            )));
        }
        let (tx, rx) = mpsc::channel();
        let mut state = self.stream.lock().unwrap();
        if state.sender.is_some() {
            return Err(LimboError::ExtensionError(format!(
                "[McpForeignDataWrapper] table '{}' already has a subscriber; a second \
                 subscription would orphan the first receiver, leaving its mirror permanently \
                 and silently stale",
                self.table_name
            )));
        }
        state.sender = Some(tx);
        state.constraints = constraints.to_vec();
        Ok(rx)
    }

    /// The declared identity columns by name, as the engine names them in its
    /// own identity errors.
    fn identity_column_list(&self) -> String {
        self.identity_columns
            .iter()
            .flatten()
            .map(|i| self.column_names[*i as usize].as_str())
            .collect::<Vec<_>>()
            .join(", ")
    }

    fn identity_values(&self, row: &[Value], identity: &[u32]) -> String {
        identity
            .iter()
            .map(|i| format!("{:?}", row[*i as usize]))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// Translate one `resources/updated` notification into `FdwChange`s and
    /// hand them to the subscriber. Returns the number of upserts emitted.
    ///
    /// UPSERT-ONLY, and it never retracts (RULED 2026-08-06). An MCP
    /// notification carries a URI and nothing else, so the re-fetch is the only
    /// source of truth available — and the re-fetch is watermark-SCOPED, which
    /// means it structurally cannot witness a deletion: a row absent from a
    /// scoped scan may be gone, or may simply belong to a parent the scope
    /// excluded, and nothing in the response distinguishes those. The driver
    /// therefore says only what it can prove: these rows exist, with these
    /// values.
    ///
    /// Convergence comes from the engine, not from a diff here: mirror upserts
    /// are identity-keyed and value-guarded, so re-pushing an unchanged row is
    /// a zero-delta no-op. Deletions are reconciled by a full REFRESH /
    /// `full_sync`, which sweeps with the guards a driver-side diff would have
    /// had to re-derive. `uri` is diagnostic only.
    pub fn push_resource_update(
        &self,
        conn: &Arc<CoreConnection>,
        uri: &str,
    ) -> Result<usize, LimboError> {
        let identity = self.identity_columns.as_deref().ok_or_else(|| {
            LimboError::ExtensionError(format!(
                "[McpForeignDataWrapper] table '{}' declares no row identity; a resource update \
                 cannot be turned into identity-keyed upserts",
                self.table_name
            ))
        })?;

        let constraints = self.stream.lock().unwrap().constraints.clone();

        debug!(
            "[McpForeignDataWrapper] re-fetching '{}' after resources/updated for {uri}",
            self.table_name
        );
        let mut cursor = self.open_cursor(conn.clone())?;
        let mut seen: HashMap<String, Vec<Value>> = HashMap::new();
        let mut upserts: Vec<FdwChange> = Vec::new();
        if cursor.filter(&constraints)? {
            loop {
                let row = (0..self.column_names.len())
                    .map(|i| cursor.column(i))
                    .collect::<Result<Vec<Value>, LimboError>>()?;
                // Guards run before anything is staged, mirroring the engine's
                // REFRESH guard: a driver that pushes what REFRESH would refuse
                // creates an inconsistency the engine cannot catch.
                if identity.iter().any(|i| row[*i as usize] == Value::Null) {
                    return Err(LimboError::Constraint(format!(
                        "foreign table '{}' returned a row whose declared identity ({}) is NULL; \
                         a NULL identity cannot be matched across scans and is not supported",
                        self.table_name,
                        self.identity_column_list()
                    )));
                }
                let key = identity_key(&row, identity);
                if seen.contains_key(&key) {
                    return Err(LimboError::Constraint(format!(
                        "foreign table '{}' returned more than one row with the same identity \
                         ({} = {}); a declared identity must identify a row uniquely within a scan",
                        self.table_name,
                        self.identity_column_list(),
                        self.identity_values(&row, identity)
                    )));
                }
                seen.insert(key, row.clone());
                upserts.push(FdwChange {
                    values: row,
                    weight: 1,
                });
                if !cursor.next()? {
                    break;
                }
            }
        }

        let count = upserts.len();
        if count > 0 {
            let state = self.stream.lock().unwrap();
            let sender = state.sender.clone().ok_or_else(|| {
                LimboError::ExtensionError(format!(
                    "[McpForeignDataWrapper] table '{}' has {count} pending changes but no \
                     subscriber; the update would be lost",
                    self.table_name
                ))
            })?;
            for change in upserts {
                sender.send(change).map_err(|e| {
                    LimboError::ExtensionError(format!(
                        "[McpForeignDataWrapper] table '{}' lost its subscriber mid-batch, {count} \
                         changes are unapplied: {e}",
                        self.table_name
                    ))
                })?;
            }
        }

        debug!(
            "[McpForeignDataWrapper] '{}' upserted {count} row(s) over identity {identity:?}",
            self.table_name
        );
        Ok(count)
    }
}

impl StreamingForeignData for McpForeignDataWrapper {
    fn subscribe(
        &self,
        constraints: &[PushedConstraint],
    ) -> Result<mpsc::Receiver<FdwChange>, LimboError> {
        McpForeignDataWrapper::subscribe(self, constraints)
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

/// Rows per INSERT OR REPLACE statement during writeback. Bounds statement
/// size so a large fan-out doesn't build one megabyte-scale SQL string.
const WRITEBACK_CHUNK_ROWS: usize = 500;

impl WritebackTarget {
    /// Write rows to the cache table via chunked INSERT OR REPLACE.
    ///
    /// DISCLOSED LIMITATION: Turso's `Connection::execute()` here exposes
    /// neither bind parameters nor an explicit-transaction seam usable from
    /// inside FDW cursor execution, so chunks run as sequential autocommit
    /// statements rather than one wrapping transaction. Each chunk is
    /// atomic; an interruption between chunks leaves a partially refreshed
    /// cache that the next refresh repairs (INSERT OR REPLACE is idempotent).
    fn write_rows(&self, rows: &[Vec<Value>]) -> Result<(), LimboError> {
        if rows.is_empty() {
            return Ok(());
        }

        let cols = self.column_names.join(", ");

        for chunk in rows.chunks(WRITEBACK_CHUNK_ROWS) {
            let value_rows: Vec<String> = chunk
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
                    "[WritebackTarget] Failed to write chunk of {} rows to '{}': {e}",
                    chunk.len(),
                    self.cache_table
                ))
            })?;
        }
        info!(
            "[WritebackTarget] Wrote {} rows to '{}' in {} chunk(s)",
            rows.len(),
            self.cache_table,
            rows.len().div_ceil(WRITEBACK_CHUNK_ROWS)
        );
        Ok(())
    }
}

struct McpCursor {
    peer: Arc<dyn McpCallSurface>,
    fetch_mode: FetchMode,
    column_to_param: HashMap<u32, String>,
    column_names: Vec<String>,
    column_configs: HashMap<String, ColumnConfig>,
    id_scheme: Option<(String, String)>,
    max_fan_out: Option<u64>,
    runtime: tokio::runtime::Handle,
    /// Database connection for enumeration enumeration queries.
    conn: Arc<CoreConnection>,
    writeback: Option<WritebackTarget>,
    /// Which parents the last `filter()` actually fetched, in row order:
    /// (enumeration param map, record count). Scopes per-parent stale-row
    /// deletion in the write-through cache.
    fan_out_groups: Vec<(HashMap<String, String>, usize)>,
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
    /// Build the tool param map from pushed constraints alone.
    fn build_tool_params(
        &self,
        constraints: &[PushedConstraint],
    ) -> serde_json::Map<String, serde_json::Value> {
        let mut params: serde_json::Map<String, serde_json::Value> = serde_json::Map::new();
        for c in constraints {
            if let Some(param_name) = self.column_to_param.get(&c.column_index) {
                params.insert(param_name.clone(), turso_value_to_json(&c.value));
            }
        }
        params
    }

    /// Build the URI template param map from defaults overlaid with
    /// constraints.
    fn build_uri_params(
        &self,
        default_params: &HashMap<String, String>,
        constraints: &[PushedConstraint],
    ) -> HashMap<String, String> {
        let mut params = default_params.clone();
        for c in constraints {
            if let Some(param_name) = self.column_to_param.get(&c.column_index)
                && let Value::Text(ref t) = c.value
            {
                params.insert(param_name.clone(), t.as_str().to_owned());
            }
        }
        params
    }

    /// One round trip to the MCP server. Returns `(records, full_response)`
    /// so callers (pagination loop) can inspect cursor / total fields.
    async fn call_tool_once_async(
        &self,
        search_tool: &str,
        extract_path: Option<&str>,
        params: serde_json::Map<String, serde_json::Value>,
    ) -> Result<
        (
            Vec<serde_json::Map<String, serde_json::Value>>,
            serde_json::Value,
        ),
        LimboError,
    > {
        info!(
            "[McpCursor] Calling tool '{}' with {} params",
            search_tool,
            params.len()
        );

        let result = self
            .peer
            .call_tool(CallToolRequestParam {
                name: Cow::Owned(search_tool.to_string()),
                arguments: Some(params),
            })
            .await
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

        let response = crate::mcp_call_surface::extract_tool_response(&result)
            .map_err(|e| LimboError::ExtensionError(format!("Failed to parse response: {e}")))?;

        let records: &Vec<serde_json::Value> = match extract_path {
            Some(path) => response
                .get(path)
                .and_then(|v| v.as_array())
                .ok_or_else(|| {
                    LimboError::ExtensionError(format!("Response missing '{path}' array"))
                })?,
            None => response.as_array().ok_or_else(|| {
                LimboError::ExtensionError(format!(
                    "Tool '{search_tool}' response is not a bare array (no extract_path set)"
                ))
            })?,
        };

        let rows = crate::mcp_sync_strategy::json_array_to_records(records)
            .map_err(|e| LimboError::ExtensionError(format!("Tool '{search_tool}': {e}")))?;
        Ok((rows, response))
    }

    /// Sync bridge for the non-fan-out path.
    fn call_tool_paginated(
        &self,
        search_tool: &str,
        extract_path: Option<&str>,
        base_params: serde_json::Map<String, serde_json::Value>,
        pagination: Option<&PaginationConfig>,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, LimboError> {
        tokio::task::block_in_place(|| {
            self.runtime.block_on(self.call_tool_paginated_async(
                search_tool,
                extract_path,
                base_params,
                pagination,
            ))
        })
    }

    /// Fetch the records for one fan-out target, looping per `PaginationConfig`
    /// when set. When `pagination` is `None`, a single call is issued.
    async fn call_tool_paginated_async(
        &self,
        search_tool: &str,
        extract_path: Option<&str>,
        base_params: serde_json::Map<String, serde_json::Value>,
        pagination: Option<&PaginationConfig>,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, LimboError> {
        let Some(pagination) = pagination else {
            let (rows, _) = self
                .call_tool_once_async(search_tool, extract_path, base_params)
                .await?;
            return Ok(rows);
        };

        let mut all = Vec::new();
        match pagination {
            PaginationConfig::Cursor {
                cursor_response_path,
                has_more_path,
                cursor_param,
                size_param,
                page_size,
            } => {
                let mut cursor: Option<String> = None;
                loop {
                    let mut p = base_params.clone();
                    if let (Some(name), Some(size)) = (size_param.as_ref(), page_size) {
                        p.insert(name.clone(), serde_json::json!(*size));
                    }
                    if let Some(c) = cursor.as_ref() {
                        p.insert(cursor_param.clone(), serde_json::Value::String(c.clone()));
                    }
                    let (rows, response) = self
                        .call_tool_once_async(search_tool, extract_path, p)
                        .await?;
                    let got = rows.len();
                    all.extend(rows);
                    if got == 0 {
                        break;
                    }
                    let has_more = response
                        .as_object()
                        .and_then(|m| resolve_json_path(m, has_more_path))
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    if !has_more {
                        break;
                    }
                    let next = response
                        .as_object()
                        .and_then(|m| resolve_json_path(m, cursor_response_path))
                        .and_then(|v| v.as_str().map(str::to_owned));
                    match next {
                        Some(c) if !c.is_empty() => cursor = Some(c),
                        _ => break,
                    }
                }
            }
            PaginationConfig::PageTotal {
                page_param,
                size_param,
                page_size,
                total_response_path,
            } => {
                let mut page: u32 = 1;
                let mut total: Option<u64> = None;
                loop {
                    let mut p = base_params.clone();
                    p.insert(page_param.clone(), serde_json::json!(page));
                    p.insert(size_param.clone(), serde_json::json!(*page_size));
                    let (rows, response) = self
                        .call_tool_once_async(search_tool, extract_path, p)
                        .await?;
                    let got = rows.len();
                    all.extend(rows);
                    if got == 0 || (got as u32) < *page_size {
                        break;
                    }
                    if total.is_none() {
                        total = response
                            .as_object()
                            .and_then(|m| resolve_json_path(m, total_response_path))
                            .and_then(|v| v.as_u64());
                    }
                    if let Some(t) = total
                        && (all.len() as u64) >= t
                    {
                        break;
                    }
                    page += 1;
                }
            }
            PaginationConfig::PageShort {
                page_param,
                size_param,
                page_size,
            } => {
                let mut page: u32 = 1;
                loop {
                    let mut p = base_params.clone();
                    p.insert(page_param.clone(), serde_json::json!(page));
                    p.insert(size_param.clone(), serde_json::json!(*page_size));
                    let (rows, _) = self
                        .call_tool_once_async(search_tool, extract_path, p)
                        .await?;
                    let got = rows.len();
                    all.extend(rows);
                    if got == 0 || (got as u32) < *page_size {
                        break;
                    }
                    page += 1;
                }
            }
        }
        Ok(all)
    }

    /// Sync bridge for the non-fan-out path.
    fn fetch_via_resource(
        &self,
        uri: &str,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, LimboError> {
        tokio::task::block_in_place(|| self.runtime.block_on(self.fetch_via_resource_async(uri)))
    }

    async fn fetch_via_resource_async(
        &self,
        uri: &str,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>, LimboError> {
        use rmcp::model::ReadResourceRequestParam;
        use rmcp::model::ResourceContents;

        info!("[McpCursor] Reading resource '{}'", uri);

        let result = self
            .peer
            .read_resource(ReadResourceRequestParam {
                uri: uri.to_string(),
            })
            .await
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

        crate::mcp_sync_strategy::json_array_to_records(records)
            .map_err(|e| LimboError::ExtensionError(format!("Resource '{uri}': {e}")))
    }
}

/// Concurrent in-flight per-parent fetches during enumeration fan-out.
/// Small constant: bounds pressure on the MCP server while hiding latency.
const FAN_OUT_CONCURRENCY: usize = 4;

impl ForeignCursor for McpCursor {
    fn filter(&mut self, constraints: &[PushedConstraint]) -> Result<bool, LimboError> {
        let mut fan_out_groups: Vec<(HashMap<String, String>, usize)> = Vec::new();
        let records = match &self.fetch_mode {
            FetchMode::Tool {
                search_tool,
                extract_path,
                static_args,
                pagination,
                enumerations,
            } => {
                let mut params = static_args.clone();
                for (k, v) in self.build_tool_params(constraints) {
                    params.insert(k, v);
                }
                let unresolved = pick_unresolved_enumerations_tool(&params, enumerations);
                match unresolved {
                    None => {
                        let mut records = self.call_tool_paginated(
                            search_tool,
                            extract_path.as_deref(),
                            params.clone(),
                            pagination.as_ref(),
                        )?;
                        stamp_call_params(&mut records, &params);
                        records
                    }
                    Some(enumeration) => {
                        let parent_rows = run_enumeration(&self.conn, enumeration)?;
                        enforce_max_fan_out(self.max_fan_out, parent_rows.len(), search_tool)?;
                        if parent_rows.is_empty() {
                            return Ok(false);
                        }
                        // Bounded-concurrency fan-out; results are collected
                        // then re-sorted by parent index so writeback ordering
                        // stays deterministic.
                        let this = &*self;
                        let mut fetched: Vec<FetchedChildRows> =
                            tokio::task::block_in_place(|| {
                                this.runtime.block_on(async {
                                    futures::stream::iter(parent_rows.into_iter().enumerate().map(
                                        |(idx, row)| {
                                            let mut p = params.clone();
                                            for (tool_param, value) in &row {
                                                p.insert(
                                                    tool_param.clone(),
                                                    serde_json::Value::String(value.clone()),
                                                );
                                            }
                                            async move {
                                                let mut records = this
                                                .call_tool_paginated_async(
                                                    search_tool,
                                                    extract_path.as_deref(),
                                                    p.clone(),
                                                    pagination.as_ref(),
                                                )
                                                .await
                                                .map_err(|e| {
                                                    LimboError::ExtensionError(format!(
                                                        "[McpCursor] tool '{search_tool}' fan-out \
                                                         failed for bindings {row:?}: {e}"
                                                    ))
                                                })?;
                                                stamp_call_params(&mut records, &p);
                                                Ok::<_, LimboError>((idx, row, records))
                                            }
                                        },
                                    ))
                                    .buffer_unordered(FAN_OUT_CONCURRENCY)
                                    .try_collect::<Vec<_>>()
                                    .await
                                })
                            })?;
                        fetched.sort_by_key(|(idx, _, _)| *idx);
                        let mut all = Vec::new();
                        for (_, row, records) in fetched {
                            fan_out_groups.push((row, records.len()));
                            all.extend(records);
                        }
                        all
                    }
                }
            }
            FetchMode::Resource { uri } => self.fetch_via_resource(uri)?,
            FetchMode::ResourceTemplate {
                template,
                default_params,
                enumerations,
            } => {
                let params = self.build_uri_params(default_params, constraints);
                let unresolved = pick_unresolved_enumerations_uri(&params, enumerations);
                match unresolved {
                    None => {
                        let uri = crate::mcp_sync_strategy::expand_uri_template(template, &params)
                            .map_err(|e| {
                                LimboError::ExtensionError(format!(
                                    "URI template param missing: {e}"
                                ))
                            })?;
                        self.fetch_via_resource(&uri)?
                    }
                    Some(enumeration) => {
                        let parent_rows = run_enumeration(&self.conn, enumeration)?;
                        enforce_max_fan_out(self.max_fan_out, parent_rows.len(), template)?;
                        if parent_rows.is_empty() {
                            return Ok(false);
                        }
                        let this = &*self;
                        let mut fetched: Vec<FetchedChildRows> =
                            tokio::task::block_in_place(|| {
                                this.runtime.block_on(async {
                                    futures::stream::iter(parent_rows.into_iter().enumerate().map(
                                        |(idx, row)| {
                                            let mut p = params.clone();
                                            for (param_name, value) in &row {
                                                p.insert(param_name.clone(), value.clone());
                                            }
                                            async move {
                                                let uri =
                                                    crate::mcp_sync_strategy::expand_uri_template(
                                                        template, &p,
                                                    )
                                                    .map_err(|e| {
                                                        LimboError::ExtensionError(format!(
                                                            "URI template param missing: {e}"
                                                        ))
                                                    })?;
                                                let records = this
                                                    .fetch_via_resource_async(&uri)
                                                    .await
                                                    .map_err(|e| {
                                                        LimboError::ExtensionError(format!(
                                                            "[McpCursor] fetch_via_resource failed \
                                                         for bindings {row:?}: {e}"
                                                        ))
                                                    })?;
                                                Ok::<_, LimboError>((idx, row, records))
                                            }
                                        },
                                    ))
                                    .buffer_unordered(FAN_OUT_CONCURRENCY)
                                    .try_collect::<Vec<_>>()
                                    .await
                                })
                            })?;
                        fetched.sort_by_key(|(idx, _, _)| *idx);
                        let mut all = Vec::new();
                        for (_, row, records) in fetched {
                            fan_out_groups.push((row, records.len()));
                            all.extend(records);
                        }
                        all
                    }
                }
            }
        };

        // Convert JSON records to rows of Turso Values, aligned with schema column
        // order. Per-column ColumnConfig (if any) drives `source_path`
        // resolution and optional JSON encoding for non-scalar values. Missing
        // fields → NULL.
        self.rows = records
            .iter()
            .map(|obj| {
                self.column_names
                    .iter()
                    .map(|col_name| {
                        let cfg = self.column_configs.get(col_name);
                        let path = cfg
                            .and_then(|c| c.source_path.as_deref())
                            .unwrap_or(col_name);
                        let raw = resolve_json_path(obj, path);
                        let val = match (raw, cfg.and_then(|c| c.encoding.as_deref())) {
                            (None, _) => Value::Null,
                            (Some(serde_json::Value::Null), _) => Value::Null,
                            (
                                Some(
                                    v
                                    @ (serde_json::Value::Array(_) | serde_json::Value::Object(_)),
                                ),
                                Some("json"),
                            ) => Value::build_text(serde_json::to_string(&v).unwrap_or_default()),
                            (Some(v), _) => json_value_to_turso_value(&v),
                        };
                        // Apply ID scheme prefix if this is the ID column
                        if let Some((ref id_col, ref scheme)) = self.id_scheme
                            && col_name == id_col
                            && let Value::Text(ref t) = val
                        {
                            return Value::build_text(format!("{scheme}:{}", t.as_str()));
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

        self.fan_out_groups = fan_out_groups;

        if let Some(ref wb) = self.writeback {
            wb.write_rows(&self.rows)?;
            if !self.fan_out_groups.is_empty() {
                self.delete_stale_children(wb, &self.fan_out_groups)?;
            }
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

/// Pick the single tool-mode enumeration whose owning param is unresolved.
///
/// Asserts at most one is unresolved — Cartesian-product fan-out of
/// independent params is not supported (and is rarely correct: correlated
/// FKs should be modeled as paired bindings in one `EnumerateFrom`).
fn pick_unresolved_enumerations_tool<'a>(
    params: &serde_json::Map<String, serde_json::Value>,
    enumerations: &'a HashMap<String, ResolvedEnumeration>,
) -> Option<&'a ResolvedEnumeration> {
    let unresolved: Vec<&ResolvedEnumeration> = enumerations
        .iter()
        .filter(|(k, _)| !params.contains_key(*k))
        .map(|(_, v)| v)
        .collect();
    assert!(
        unresolved.len() <= 1,
        "Multiple independent enumerate_from sources not supported (use paired `fields:` for \
         correlated FKs); got {} unresolved",
        unresolved.len()
    );
    unresolved.into_iter().next()
}

/// URI-template variant of [`pick_unresolved_enumerations_tool`].
fn pick_unresolved_enumerations_uri<'a>(
    params: &HashMap<String, String>,
    enumerations: &'a HashMap<String, ResolvedEnumeration>,
) -> Option<&'a ResolvedEnumeration> {
    let unresolved: Vec<&ResolvedEnumeration> = enumerations
        .iter()
        .filter(|(k, _)| !params.contains_key(*k))
        .map(|(_, v)| v)
        .collect();
    assert!(
        unresolved.len() <= 1,
        "Multiple independent enumerate_from sources not supported (use paired `fields:` for \
         correlated FKs); got {} unresolved",
        unresolved.len()
    );
    unresolved.into_iter().next()
}

/// Run an enumeration SQL query and return one binding-map per row.
///
/// Each row's columns are aligned with `enumeration.bindings` and emitted as
/// `tool_param_name → value`. For legacy single-field bindings, every row
/// produces a one-entry map. For paired bindings (e.g. `owner` + `repo`),
/// every row produces a multi-entry map so all correlated params get bound
/// together in the fan-out call.
fn run_enumeration(
    conn: &Arc<CoreConnection>,
    enumeration: &ResolvedEnumeration,
) -> Result<Vec<HashMap<String, String>>, LimboError> {
    info!(
        "[McpCursor] Running enumeration: {}",
        enumeration.enumerate_sql
    );
    let mut stmt = conn.query(&enumeration.enumerate_sql)?.ok_or_else(|| {
        LimboError::ExtensionError(format!(
            "Enumeration query returned no statement: {}",
            enumeration.enumerate_sql
        ))
    })?;

    let n_cols = enumeration.bindings.len();
    let mut rows = Vec::new();
    loop {
        match stmt.step()? {
            turso_core::StepResult::Row => {
                if let Some(row) = stmt.row() {
                    let mut binding_map = HashMap::with_capacity(n_cols);
                    for (col_idx, binding) in enumeration.bindings.iter().enumerate() {
                        let s = match row.get_value(col_idx) {
                            Value::Text(t) => t.as_str().to_owned(),
                            Value::Numeric(turso_core::Numeric::Integer(i)) => i.to_string(),
                            Value::Numeric(turso_core::Numeric::Float(f)) => format!("{}", *f),
                            Value::Null => {
                                // A NULL FK column for an FK enumeration is a data error,
                                // not a fan-out target. Fail loud rather than silently
                                // calling the tool with a missing param.
                                return Err(LimboError::ExtensionError(format!(
                                    "Enumeration '{}' produced NULL for column '{}' (tool param \
                                     '{}') — refusing to fan out with missing param",
                                    enumeration.enumerate_sql,
                                    binding.parent_col,
                                    binding.tool_param
                                )));
                            }
                            Value::Blob(_) => {
                                return Err(LimboError::ExtensionError(format!(
                                    "Enumeration '{}' produced BLOB for column '{}' — not \
                                     coercible to a tool param",
                                    enumeration.enumerate_sql, binding.parent_col
                                )));
                            }
                        };
                        // Boundary conversion (single place): cached ids are
                        // scheme-prefixed; MCP tool/URI params expect raw ids.
                        let s = match &binding.strip_scheme {
                            None => s,
                            Some(scheme) => {
                                let expected = format!("{scheme}:");
                                match s.strip_prefix(expected.as_str()) {
                                    Some(raw) => raw.to_owned(),
                                    None => {
                                        return Err(LimboError::ExtensionError(format!(
                                            "Enumeration '{}' produced id '{}' for column '{}' \
                                             without the expected '{}' scheme prefix — cached ids \
                                             must be scheme-prefixed",
                                            enumeration.enumerate_sql,
                                            s,
                                            binding.parent_col,
                                            expected
                                        )));
                                    }
                                }
                            }
                        };
                        binding_map.insert(binding.tool_param.clone(), s);
                    }
                    rows.push(binding_map);
                }
            }
            turso_core::StepResult::Done => break,
            turso_core::StepResult::IO => continue,
            _ => break,
        }
    }

    info!("[McpCursor] Enumeration produced {} rows", rows.len());
    Ok(rows)
}

/// Fail loud when an enumeration exceeds the declared fan-out bound.
/// No silent truncation — the config either raises `max_fan_out`, narrows the
/// enumeration with `where`/`limit`, or accepts the full fan-out.
fn enforce_max_fan_out(
    max_fan_out: Option<u64>,
    count: usize,
    target: &str,
) -> Result<(), LimboError> {
    if let Some(max) = max_fan_out
        && count as u64 > max
    {
        return Err(LimboError::ExtensionError(format!(
            "[McpCursor] enumeration for '{target}' produced {count} fan-out targets, exceeding \
             max_fan_out={max} — refusing to fan out (no silent truncation); raise max_fan_out or \
             narrow the enumeration with where/limit"
        )));
    }
    Ok(())
}

impl McpCursor {
    /// Per-parent stale-row deletion after a fan-out refresh.
    ///
    /// SCOPING RULE: for each refreshed parent P (and ONLY refreshed
    /// parents — never global), delete cache rows whose parent-key columns
    /// equal P's enumeration values and whose id is not among the freshly
    /// fetched ids. Applicable only when every enumeration param maps to a
    /// declared schema column (the vtable's parent-key columns) and the id
    /// column is declared via `id_scheme`; otherwise deletion is skipped —
    /// disclosed at debug level — because the cache rows can't be scoped.
    fn delete_stale_children(
        &self,
        wb: &WritebackTarget,
        fan_out_groups: &[(HashMap<String, String>, usize)],
    ) -> Result<(), LimboError> {
        let Some((id_col, _)) = self.id_scheme.as_ref() else {
            tracing::debug!(
                "[McpCursor] no id_scheme declared for '{}' — skipping stale-row deletion",
                wb.cache_table
            );
            return Ok(());
        };
        let Some(id_idx) = self.column_names.iter().position(|c| c == id_col) else {
            return Err(LimboError::ExtensionError(format!(
                "[McpCursor] id column '{id_col}' not in schema columns {:?}",
                self.column_names
            )));
        };
        let param_to_column: HashMap<&str, &str> = self
            .column_to_param
            .iter()
            .map(|(idx, param)| (param.as_str(), self.column_names[*idx as usize].as_str()))
            .collect();

        let mut offset = 0usize;
        for (param_map, len) in fan_out_groups {
            let range = offset..offset + len;
            offset += len;

            let mut predicates = Vec::with_capacity(param_map.len());
            for (param, value) in param_map {
                let Some(col) = param_to_column.get(param.as_str()) else {
                    tracing::debug!(
                        "[McpCursor] enumeration param '{param}' has no schema column in '{}' — \
                         parent-key not declared, skipping stale-row deletion",
                        wb.cache_table
                    );
                    return Ok(());
                };
                predicates.push(format!(
                    "{col} = {}",
                    value_to_sql_literal(&Value::build_text(value.clone()))
                ));
            }

            let fresh_ids: Vec<String> = self.rows[range]
                .iter()
                .map(|row| {
                    let id = &row[id_idx];
                    if matches!(id, Value::Null) {
                        return Err(LimboError::ExtensionError(format!(
                            "[McpCursor] fetched row has NULL id column '{id_col}' — cannot scope \
                             stale-row deletion in '{}'",
                            wb.cache_table
                        )));
                    }
                    Ok(value_to_sql_literal(id))
                })
                .collect::<Result<_, _>>()?;

            let mut sql = format!(
                "DELETE FROM {} WHERE {}",
                wb.cache_table,
                predicates.join(" AND ")
            );
            if !fresh_ids.is_empty() {
                sql.push_str(&format!(" AND {id_col} NOT IN ({})", fresh_ids.join(", ")));
            }
            wb.conn.execute(&sql).map_err(|e| {
                LimboError::ExtensionError(format!(
                    "[McpCursor] stale-row deletion failed in '{}': {e} (sql: {sql})",
                    wb.cache_table
                ))
            })?;
        }
        Ok(())
    }
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

/// Inject every (param, value) into each record under `param` as the key,
/// without clobbering existing fields. Lets SQL columns named after tool
/// params (e.g. `owner`, `repo` for `list_issues`) carry the request-time
/// value when the response doesn't echo it.
fn stamp_call_params(
    records: &mut [serde_json::Map<String, serde_json::Value>],
    params: &serde_json::Map<String, serde_json::Value>,
) {
    for record in records.iter_mut() {
        for (k, v) in params {
            record.entry(k.clone()).or_insert_with(|| v.clone());
        }
    }
}

/// Resolve a dotted JSON path against a record object. Returns `Some(value)`
/// when every segment exists (including `Null`), or `None` when any segment
/// is missing or traverses through a non-object.
fn resolve_json_path(
    obj: &serde_json::Map<String, serde_json::Value>,
    path: &str,
) -> Option<serde_json::Value> {
    let mut segments = path.split('.');
    let first = segments.next()?;
    let mut current = obj.get(first)?.clone();
    for seg in segments {
        current = match current {
            serde_json::Value::Object(ref m) => m.get(seg)?.clone(),
            _ => return None,
        };
    }
    Some(current)
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

        // Structured enumerate_from → dynamic (with enumeration)
        let yaml_enumeration = r#"
list_resource: "claude-history://sessions/{session_id}/messages"
uri_params:
  session_id:
    enumerate_from:
      entity: session
      field: id
"#;
        let config_fb: VtableConfig = serde_yaml::from_str(yaml_enumeration).unwrap();
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

        // session_id should be registered but NOT required (has enumeration)
        assert_eq!(key_columns.len(), 1);
        let kc = &key_columns[0];
        assert_eq!(kc.name, "session_id");
        assert!(!kc.required);
        assert_eq!(column_to_param.get(&1), Some(&"session_id".to_string()));
    }

    #[test]
    fn enumerate_from_legacy_single_field_parses() {
        let yaml = r#"
entity: session
field: id
"#;
        let ef: EnumerateFrom = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(ef.entity, "session");
        assert_eq!(ef.field.as_deref(), Some("id"));
        assert!(ef.fields.is_none());
        // Owning param is used as the binding target for legacy shape.
        let bindings = ef.raw_bindings("session_id");
        assert_eq!(bindings, vec![("id".to_string(), "session_id".to_string())]);
    }

    #[test]
    fn enumerate_from_paired_fields_parses() {
        let yaml = r#"
entity: repository
fields:
  owner: owner
  repo:  name
"#;
        let ef: EnumerateFrom = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(ef.entity, "repository");
        assert!(ef.field.is_none());
        let map = ef.fields.as_ref().unwrap();
        assert_eq!(map.get("owner"), Some(&"owner".to_string()));
        assert_eq!(map.get("repo"), Some(&"name".to_string()));
        let mut bindings = ef.raw_bindings("ignored");
        bindings.sort();
        let mut expected = vec![
            ("name".to_string(), "repo".to_string()),
            ("owner".to_string(), "owner".to_string()),
        ];
        expected.sort();
        assert_eq!(bindings, expected);
    }

    #[test]
    fn resolved_enumeration_sql_for_paired_fields() {
        let ef = EnumerateFrom {
            entity: "repository".to_string(),
            field: None,
            fields: Some({
                let mut m = HashMap::new();
                m.insert("owner".to_string(), "owner".to_string());
                m.insert("repo".to_string(), "name".to_string());
                m
            }),
            where_sql: None,
            order_by: None,
            limit: None,
        };
        let r = ResolvedEnumeration::from_enumerate_from(&ef, "ignored", "gh_", "id");
        // HashMap iteration order is nondeterministic, so check both possibilities.
        assert!(
            r.enumerate_sql == "SELECT owner, name FROM gh_repository"
                || r.enumerate_sql == "SELECT name, owner FROM gh_repository",
            "got: {}",
            r.enumerate_sql
        );
        assert_eq!(r.bindings.len(), 2);
    }

    #[test]
    fn filter_mapping_with_paired_enumerate_from_parses() {
        let yaml = r#"
search_tool: list_issues
extract_path: issues
filter_mapping:
  owner:
    param: owner
    ops: ["="]
    enumerate_from:
      entity: repository
      fields:
        owner: owner
        repo:  name
  repo:
    param: repo
    ops: ["="]
"#;
        let config: VtableConfig = serde_yaml::from_str(yaml).unwrap();
        let owner = &config.filter_mapping["owner"];
        assert!(owner.enumerate_from.is_some());
        let ef = owner.enumerate_from.as_ref().unwrap();
        assert_eq!(ef.entity, "repository");
        let paired = ef.fields.as_ref().unwrap();
        assert_eq!(paired.get("repo"), Some(&"name".to_string()));

        let repo = &config.filter_mapping["repo"];
        assert!(repo.enumerate_from.is_none());
    }

    #[test]
    fn paired_enumeration_marks_both_params_non_required() {
        // Even with `required: true` on the YAML, paired-bound params must
        // be non-required at the KeyColumn level — the enumeration supplies them.
        let yaml = r#"
search_tool: list_issues
extract_path: issues
filter_mapping:
  owner:
    param: owner
    ops: ["="]
    required: true
    enumerate_from:
      entity: repository
      fields:
        owner: owner
        repo:  name
  repo:
    param: repo
    ops: ["="]
    required: true
"#;
        let config: VtableConfig = serde_yaml::from_str(yaml).unwrap();
        let columns = vec![
            ("id".to_string(), "TEXT".to_string()),
            ("owner".to_string(), "TEXT".to_string()),
            ("repo".to_string(), "TEXT".to_string()),
        ];
        let (key_columns, _, _, _) = build_fdw_metadata("gh_issue", &columns, &config);
        let owner_kc = key_columns.iter().find(|kc| kc.name == "owner").unwrap();
        let repo_kc = key_columns.iter().find(|kc| kc.name == "repo").unwrap();
        assert!(
            !owner_kc.required,
            "owner owns an enumerate_from — must be non-required"
        );
        assert!(
            !repo_kc.required,
            "repo is paired-bound via owner's enumerate_from — must be non-required"
        );
    }

    #[test]
    fn tool_fetch_mode_carries_enumerations() {
        let yaml = r#"
search_tool: list_issues
extract_path: issues
filter_mapping:
  owner:
    param: owner
    ops: ["="]
    enumerate_from:
      entity: repository
      fields:
        owner: owner
        repo:  name
  repo:
    param: repo
    ops: ["="]
"#;
        let config: VtableConfig = serde_yaml::from_str(yaml).unwrap();
        let columns = vec![
            ("id".to_string(), "TEXT".to_string()),
            ("owner".to_string(), "TEXT".to_string()),
            ("repo".to_string(), "TEXT".to_string()),
        ];
        // Drive McpForeignDataWrapper construction up to the point where
        // FetchMode is built. We can't call `new()` without a Peer, so
        // exercise the build path via internal logic by re-deriving here.
        let prefix = "gh_";
        let enumerations: HashMap<String, ResolvedEnumeration> = config
            .filter_mapping
            .values()
            .filter_map(|fc| {
                fc.enumerate_from.as_ref().map(|ef| {
                    (
                        fc.param.clone(),
                        ResolvedEnumeration::from_enumerate_from(ef, &fc.param, prefix, "id"),
                    )
                })
            })
            .collect();
        // One entry: keyed by the owning param `owner`.
        assert_eq!(enumerations.len(), 1);
        let owner_enum = enumerations.get("owner").expect("owner enumeration");
        assert_eq!(owner_enum.bindings.len(), 2);
        assert!(owner_enum.enumerate_sql.contains("FROM gh_repository"));

        // Sanity: build_fdw_metadata also registers `repo` as a key column
        // (so SQL `WHERE repo='x'` would pushdown), but non-required.
        let (key_columns, column_to_param, _, _) =
            build_fdw_metadata("gh_issue", &columns, &config);
        assert_eq!(key_columns.len(), 2);
        assert_eq!(column_to_param.get(&1), Some(&"owner".to_string()));
        assert_eq!(column_to_param.get(&2), Some(&"repo".to_string()));
    }

    #[test]
    fn pick_unresolved_tool_returns_single() {
        let mut enumerations = HashMap::new();
        let ef = EnumerateFrom {
            entity: "repository".to_string(),
            field: None,
            fields: Some({
                let mut m = HashMap::new();
                m.insert("owner".to_string(), "owner".to_string());
                m.insert("repo".to_string(), "name".to_string());
                m
            }),
            where_sql: None,
            order_by: None,
            limit: None,
        };
        enumerations.insert(
            "owner".to_string(),
            ResolvedEnumeration::from_enumerate_from(&ef, "owner", "gh_", "id"),
        );

        // Empty params → unresolved
        let empty = serde_json::Map::new();
        assert!(pick_unresolved_enumerations_tool(&empty, &enumerations).is_some());

        // owner present → resolved
        let mut filled = serde_json::Map::new();
        filled.insert(
            "owner".to_string(),
            serde_json::Value::String("martinmauch".to_string()),
        );
        assert!(pick_unresolved_enumerations_tool(&filled, &enumerations).is_none());
    }

    #[test]
    #[should_panic(expected = "Multiple independent enumerate_from sources")]
    fn pick_unresolved_tool_rejects_multiple_independent() {
        // Two independent enumerate_from entries (Cartesian product would be
        // wrong for correlated FKs). Helper asserts.
        let make = |entity: &str, field: &str| EnumerateFrom {
            entity: entity.to_string(),
            field: Some(field.to_string()),
            fields: None,
            where_sql: None,
            order_by: None,
            limit: None,
        };
        let mut enumerations = HashMap::new();
        enumerations.insert(
            "a".to_string(),
            ResolvedEnumeration::from_enumerate_from(&make("ent_a", "id"), "a", "", "id"),
        );
        enumerations.insert(
            "b".to_string(),
            ResolvedEnumeration::from_enumerate_from(&make("ent_b", "id"), "b", "", "id"),
        );
        let empty = serde_json::Map::new();
        let _ = pick_unresolved_enumerations_tool(&empty, &enumerations);
    }

    #[test]
    fn enumeration_sql_with_where_order_limit() {
        let yaml = r#"
entity: session
field: id
where: "modified > '2026-01-01'"
order_by: modified DESC
limit: 20
"#;
        let ef: EnumerateFrom = serde_yaml::from_str(yaml).unwrap();
        let r = ResolvedEnumeration::from_enumerate_from(&ef, "session_id", "cc_", "id");
        assert_eq!(
            r.enumerate_sql,
            "SELECT id FROM cc_session WHERE modified > '2026-01-01' ORDER BY modified DESC LIMIT \
             20"
        );
    }

    #[test]
    fn enumeration_id_binding_carries_strip_scheme() {
        let ef = EnumerateFrom {
            entity: "session".to_string(),
            field: Some("id".to_string()),
            fields: None,
            where_sql: None,
            order_by: None,
            limit: None,
        };
        let r = ResolvedEnumeration::from_enumerate_from(&ef, "session_id", "cc_", "id");
        assert_eq!(r.bindings.len(), 1);
        // Scheme uses EntityName normalization: underscores → hyphens.
        assert_eq!(r.bindings[0].strip_scheme.as_deref(), Some("cc-session"));

        // Non-id parent columns carry no scheme.
        let ef2 = EnumerateFrom {
            entity: "repository".to_string(),
            field: Some("owner".to_string()),
            fields: None,
            where_sql: None,
            order_by: None,
            limit: None,
        };
        let r2 = ResolvedEnumeration::from_enumerate_from(&ef2, "owner", "gh_", "id");
        assert_eq!(r2.bindings[0].strip_scheme, None);
    }

    #[test]
    fn max_fan_out_exceeded_is_loud() {
        let err = enforce_max_fan_out(Some(2), 3, "list_messages").unwrap_err();
        let msg = format!("{err}");
        assert!(msg.contains("max_fan_out=2"), "names the limit: {msg}");
        assert!(msg.contains("3 fan-out targets"), "names the count: {msg}");
        enforce_max_fan_out(Some(3), 3, "list_messages").expect("at the limit is fine");
        enforce_max_fan_out(None, 10_000, "list_messages").expect("unbounded");
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

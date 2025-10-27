use holon_api::QueryLanguage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CreateTableParams {
    pub table_name: String,
    pub columns: Vec<ColumnDef>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ColumnDef {
    pub name: String,
    pub sql_type: String, // TEXT, INTEGER, BOOLEAN, etc.
    #[serde(default)]
    pub primary_key: bool,
    pub default: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct InsertDataParams {
    pub table_name: String,
    pub rows: Vec<HashMap<String, serde_json::Value>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecuteQueryParams {
    /// The query string to execute
    pub query: String,
    /// Query language: "holon_prql", "holon_gql", or "holon_sql"
    pub language: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    /// Block ID for `from children` context resolution. When set, `from children` returns
    /// children of this block. Without this, `from children` returns empty results.
    pub context_id: Option<String>,
    /// Parent block ID for `from siblings` context resolution.
    pub context_parent_id: Option<String>,
    /// Render spec for GQL/SQL queries. Parsed as PRQL render expression.
    /// Example: "list item_template:(row (text this.name))"
    pub render: Option<String>,
    /// When true, each row gets a `_profile` key with resolved entity profile info
    /// (profile name, render expression, available operations).
    #[serde(default)]
    pub include_profile: Option<bool>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecuteOperationParams {
    pub entity_name: String,
    pub operation: String,
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct WatchQueryParams {
    /// The query string to watch
    pub query: String,
    /// Query language: "holon_prql", "holon_gql", or "holon_sql". Defaults to "holon_prql".
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    /// Render spec for GQL/SQL queries
    pub render: Option<String>,
}

fn default_language() -> String {
    QueryLanguage::HolonPrql.to_string()
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct WatchHandle {
    pub watch_id: String,
    pub initial_data: Vec<HashMap<String, serde_json::Value>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct QueryResult {
    pub rows: Vec<HashMap<String, serde_json::Value>>,
    pub row_count: usize,
    /// Query execution time in milliseconds (wall clock, excluding serialization).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RowChangeJson {
    pub change_type: String, // "Created", "Updated", "Deleted"
    pub entity_id: Option<String>,
    pub data: Option<HashMap<String, serde_json::Value>>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DropTableParams {
    pub table_name: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ListOperationsParams {
    pub entity_name: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct StopWatchParams {
    pub watch_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct PollChangesParams {
    pub watch_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RankTasksResult {
    pub tasks: Vec<RankedTaskJson>,
    pub mental_slots: MentalSlotsJson,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RankedTaskJson {
    pub rank: usize,
    pub block_id: String,
    pub label: String,
    pub delta_obj: f64,
    pub delta_per_minute: f64,
    pub duration_minutes: f64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct MentalSlotsJson {
    pub occupied: usize,
    pub capacity: usize,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct UndoRedoResult {
    pub success: bool,
    pub message: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CanUndoRedoResult {
    pub available: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecuteRawSqlParams {
    /// Raw SQL to send directly to Turso. No PRQL/GQL compilation, no SQL transforms.
    pub sql: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

// --- Debug tool types ---

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CompileQueryParams {
    /// The query string to compile
    pub query: String,
    /// Query language: "holon_prql", "holon_gql", or "holon_sql"
    pub language: String,
    /// Optional render spec (for GQL/SQL queries)
    pub render: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CompileQueryResult {
    pub compiled_sql: String,
    pub render_spec: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct InspectLoroBlocksParams {
    /// Document ID — can be a UUID or a file path
    pub doc_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DiffLoroSqlParams {
    /// Document ID — can be a UUID or a file path
    pub doc_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ReadOrgFileParams {
    /// Document ID — can be a UUID or a file path. Resolved to file path via aliases.
    pub doc_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RenderOrgParams {
    /// Document ID — can be a UUID or a file path
    pub doc_id: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DescribeUiParams {
    /// Block ID to render and describe
    pub block_id: String,
    /// Output format: "text" for pretty-printed tree, "json" for structured JSON
    #[serde(default = "default_text_format")]
    pub format: String,
}

fn default_text_format() -> String {
    "text".to_string()
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotParams {
    /// Window title or app name substring to match (e.g. "Holon" for GPUI, "Blinc").
    /// If omitted, tries known frontend names in order: "Holon", "Blinc".
    pub window_title: Option<String>,
}

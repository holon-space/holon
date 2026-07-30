use std::collections::HashMap;

use holon_api::QueryLanguage;
use schemars::JsonSchema;
use serde::Deserialize;
use serde::Serialize;

/// Wrapper for TypeDefinition JSON — the tool description explains the shape.
/// We use serde_json::Value because TypeDefinition doesn't derive JsonSchema.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CreateEntityTypeParams {
    /// TypeDefinition as JSON object. Shape: {name, fields: [{name, sql_type,
    /// ...}], primary_key?, graph_label?, id_references?}
    pub type_definition: serde_json::Value,
}

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

/// Parameters for `dense_query` — a token-compressed org projection of a
/// query's block result, returned in one call with an opaque handle for a later
/// `dense_patch`. Mirrors `execute_query`: the CALLER writes the filter as an
/// ordinary GQL/PRQL/SQL query (exclude DONE, scope to a subtree, limit depth —
/// all in the query language), and the tool renders the resulting blocks
/// densely. There are no Rust-side filter parameters.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DenseQueryParams {
    /// The query. It MUST return block rows (`SELECT * FROM block ...` in
    /// holon_sql, or a GQL/PRQL query over blocks) — every column is parsed
    /// into a Block for rendering.
    pub query: String,
    /// Query language: "holon_prql", "holon_gql", or "holon_sql".
    pub language: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    /// Block ID for `from children` / `from descendants` context resolution.
    pub context_id: Option<String>,
    /// Parent block ID for `from siblings` context resolution.
    pub context_parent_id: Option<String>,
}

/// Parameters for `dense_patch` — apply an edited dense projection back as a
/// batch of block operations, matched by `{#alias}` handle.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DensePatchParams {
    /// The `projection_handle` returned by `dense_query`.
    pub handle: String,
    /// The edited dense org text. Rows keep their `{#alias}` token to match an
    /// existing block; a row with NO token is created as a NEW block at its
    /// tree position. Blocks omitted from the text are NOT deleted.
    pub text: String,
    /// Aliases to delete explicitly (deletion is never inferred from omission).
    #[serde(default)]
    pub delete: Vec<String>,
    /// When true, only report the planned operations and any conflicts without
    /// applying them.
    #[serde(default)]
    pub dry_run: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecuteQueryParams {
    /// The query string to execute
    pub query: String,
    /// Query language: "holon_prql", "holon_gql", or "holon_sql"
    pub language: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    /// Block ID for `from children` context resolution. When set, `from
    /// children` returns children of this block. Without this, `from
    /// children` returns empty results.
    pub context_id: Option<String>,
    /// Parent block ID for `from siblings` context resolution.
    pub context_parent_id: Option<String>,
    /// Render spec for GQL/SQL queries. Parsed as PRQL render expression.
    /// Example: "list item_template:(row (text this.name))"
    pub render: Option<String>,
    /// When true, each row gets a `_profile` key with resolved entity profile
    /// info (profile name, render expression, available operations).
    #[serde(default)]
    pub include_profile: Option<bool>,
    /// Output encoding: `"toon"` (default) or `"json"`. TOON is a dense tabular
    /// text (`name[N]{cols}: rows…`) that drops the repeated per-row key names
    /// — biggest savings on wide, uniform result sets. Pass `"json"` if you
    /// need plain JSON rows (e.g. rows dominated by nested JSON blobs).
    #[serde(default)]
    pub format: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecuteSourceBlockParams {
    /// Block ID of a source block whose `content` is the query and whose
    /// `source_language` is one of `holon_prql` / `holon_gql` / `holon_sql`.
    /// Bare slugs are accepted (auto-prefixed to `block:`).
    pub block_id: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    /// Override `source_language`. When omitted, the block's stored
    /// `source_language` is used.
    pub language: Option<String>,
    /// Block ID for `from children` context resolution.
    pub context_id: Option<String>,
    /// Parent block ID for `from siblings` context resolution.
    pub context_parent_id: Option<String>,
    /// Render spec override (mirrors `execute_query`).
    pub render: Option<String>,
    /// When true, each row gets a `_profile` key with resolved entity profile
    /// info.
    #[serde(default)]
    pub include_profile: Option<bool>,
    /// Output encoding: `"toon"` (default, dense tabular) or `"json"` (mirrors
    /// `execute_query`).
    #[serde(default)]
    pub format: Option<String>,
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
    /// Query language: "holon_prql", "holon_gql", or "holon_sql". Defaults to
    /// "holon_prql".
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
    /// Query execution time in milliseconds (wall clock, excluding
    /// serialization).
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
    /// Raw SQL to send directly to Turso. No PRQL/GQL compilation, no SQL
    /// transforms.
    pub sql: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
    /// Output encoding: `"toon"` (default, dense tabular) or `"json"` (mirrors
    /// `execute_query`).
    #[serde(default)]
    pub format: Option<String>,
}

/// Filter for the `query_history` tool (C2b op/effect history, ADR 0024 P8).
/// Every field mirrors `holon_api::HistoryQuery`; `count` returns the match
/// count instead of the rows. `deny_unknown_fields` makes an unknown/misspelled
/// filter key a LOUD error at the boundary (parse-don't-validate) rather than a
/// silently-ignored filter that would return the wrong rows.
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct QueryHistoryParams {
    /// Entity type the op ran on (e.g. `block`).
    #[serde(default)]
    pub entity_name: Option<String>,
    /// The affected block id.
    #[serde(default)]
    pub block_id: Option<String>,
    /// Provenance origin tag (`user` / `agent` / `rule` / `sync` / ...).
    #[serde(default)]
    pub origin: Option<String>,
    /// Driving agent session id.
    #[serde(default)]
    pub session_id: Option<String>,
    /// The field that changed.
    #[serde(default)]
    pub field: Option<String>,
    /// The new field value (the "moved to X" predicate).
    #[serde(default)]
    pub new_value: Option<String>,
    /// UTC calendar day (`YYYY-MM-DD`).
    #[serde(default)]
    pub day: Option<String>,
    /// All events of one op group.
    #[serde(default)]
    pub op_group: Option<i64>,
    /// Inclusive lower bound on `at_millis`.
    #[serde(default)]
    pub since_millis: Option<i64>,
    /// Exclusive upper bound on `at_millis`.
    #[serde(default)]
    pub until_millis: Option<i64>,
    /// Return the match count instead of the event rows.
    #[serde(default)]
    pub count: bool,
}

impl From<QueryHistoryParams> for holon_api::HistoryQueryArgs {
    fn from(p: QueryHistoryParams) -> Self {
        holon_api::HistoryQueryArgs {
            entity_name: p.entity_name,
            block_id: p.block_id,
            origin: p.origin,
            session_id: p.session_id,
            field: p.field,
            new_value: p.new_value,
            day: p.day,
            op_group: p.op_group,
            since_millis: p.since_millis,
            until_millis: p.until_millis,
            count: p.count,
        }
    }
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
    /// Document ID — can be a UUID or a file path. Resolved to file path via
    /// aliases.
    pub doc_id: String,
}

/// Which store the blocks are read from.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RenderSource {
    /// The SQL write authority — the state write-back projects to disk.
    #[default]
    Sql,
    /// The Loro CRDT tree.
    Loro,
}

/// How much of the file the render covers.
#[derive(Serialize, Deserialize, JsonSchema, Default, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum RenderScope {
    /// The whole file: document header (`#+TITLE:`, `#+ID:`) plus body.
    #[default]
    Document,
    /// The body alone, no header.
    Blocks,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RenderOrgParams {
    /// Document ID — can be a UUID or a file path
    pub doc_id: String,
    #[serde(default)]
    pub source: RenderSource,
    #[serde(default)]
    pub scope: RenderScope,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct DescribeUiParams {
    /// Block ID to render and describe
    pub block_id: String,
    /// Output format: "text" for pretty-printed tree, "json" for structured
    /// JSON
    #[serde(default = "default_text_format")]
    pub format: String,
}

fn default_text_format() -> String {
    "text".to_string()
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ResetVaultFile {
    /// File name (e.g. `"structural-page.org"`). Its stem becomes a Page in the
    /// left sidebar.
    pub name: String,
    /// Org file body.
    pub content: String,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ResetVaultParams {
    /// Seed `.org` files to materialize into a FRESH temp vault. The running
    /// window is rebound onto the freshly-booted engine in place — no second
    /// MCP server, no window relaunch. Client supplies the seed so the server
    /// embeds no seed copy (single source of truth).
    pub files: Vec<ResetVaultFile>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct RowDropLedgersParams {
    /// Clear all three ledgers AFTER reading them, so the next read reflects
    /// only what happened since. Defaults to false.
    #[serde(default)]
    pub reset: bool,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct AwaitQuiescenceParams {
    /// Upper bound on the combined-fixed-point wait, in milliseconds. When the
    /// budget is exhausted before every reachable signal is simultaneously
    /// stable, the tool returns an error naming the still-moving signal(s) —
    /// it never reports a non-converged wait as success. Defaults to 30000.
    #[serde(default)]
    pub budget_ms: Option<u64>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ScreenshotParams {
    /// Window title or app name substring to match (e.g. "Holon" for GPUI,
    /// "Blinc"). If omitted, tries known frontend names in order: "Holon",
    /// "Blinc".
    pub window_title: Option<String>,
}

// --- UI interaction tool types ---

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SendNavigationParams {
    /// Entity ID of the element to navigate from (e.g. a block's row_id).
    pub from_entity_id: String,
    /// Navigation direction: "up" or "down".
    pub direction: String,
    /// Optional cursor column hint for placement in the target block.
    #[serde(default)]
    pub cursor_column: Option<usize>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct SendKeyChordParams {
    /// Entity ID of the element to target (key chord bubbles up from here).
    pub entity_id: String,
    /// Keys in the chord, e.g. ["cmd", "enter"] or ["shift", "tab"].
    pub keys: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ListCommandsParams {
    /// Block ID to list available commands for.
    pub block_id: String,
    /// Optional filter string to narrow commands.
    #[serde(default)]
    pub filter: Option<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ExecuteCommandParams {
    /// Block ID to execute the command on.
    pub block_id: String,
    /// Command name (operation name from list_commands).
    pub command_name: String,
    /// Entity name for the operation (e.g. "blocks").
    pub entity_name: String,
    /// Additional parameters for the command.
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ClickParams {
    /// Optional entity id (block id). When set, the click is dispatched at
    /// the center of the rendered element via the same entity-addressed
    /// `UserDriver::click_entity` path the PBT/E2E tests use — it resolves
    /// the element bounds, hit-tests the point, and warns if a different
    /// element is on top. `x`/`y`/`button`/`modifiers` are ignored (the
    /// driver synthesizes a plain left click). Prefer this over raw
    /// coordinates: it survives relayout/scroll and self-verifies the hit.
    #[serde(default)]
    pub entity_id: Option<String>,
    /// Panel region for the entity click, e.g. "main", "left_sidebar".
    /// Only used with `entity_id`. Defaults to "main".
    #[serde(default = "default_main")]
    pub region: String,
    /// X coordinate in logical pixels. Ignored if `entity_id` is set.
    #[serde(default)]
    pub x: f32,
    /// Y coordinate in logical pixels. Ignored if `entity_id` is set.
    #[serde(default)]
    pub y: f32,
    /// Mouse button: "left" (default), "right", "middle". Ignored if
    /// `entity_id` is set.
    #[serde(default = "default_left")]
    pub button: String,
    /// Modifier keys held during click, e.g. ["cmd", "shift"]. Ignored if
    /// `entity_id` is set.
    #[serde(default)]
    pub modifiers: Vec<String>,
}

fn default_left() -> String {
    "left".to_string()
}

fn default_main() -> String {
    "main".to_string()
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ScrollParams {
    /// Optional entity id (block id). When set, the scroll event is
    /// dispatched at the center of the rendered element; `x`/`y` are
    /// ignored. When omitted, `x`/`y` are required.
    #[serde(default)]
    pub entity_id: Option<String>,
    /// X coordinate in logical pixels. Ignored if `entity_id` is set.
    #[serde(default)]
    pub x: f32,
    /// Y coordinate in logical pixels. Ignored if `entity_id` is set.
    #[serde(default)]
    pub y: f32,
    /// Horizontal scroll delta in lines. Positive = scroll right.
    #[serde(default)]
    pub dx: f32,
    /// Vertical scroll delta in lines. Positive = scroll down.
    #[serde(default)]
    pub dy: f32,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct TypeTextParams {
    /// Text to type, or a special key name (e.g. "enter", "tab", "escape").
    pub text: String,
    /// Modifier keys held during typing, e.g. ["cmd", "shift"].
    #[serde(default)]
    pub modifiers: Vec<String>,
}

#[derive(Serialize, Deserialize, JsonSchema)]
pub struct InsertTextParams {
    /// Text to insert via the soft-keyboard `insertText:` path (bypasses the
    /// GPUI keymap and commits straight into the focused editor). A soft
    /// Return is `"\n"` and is translated to an `enter` action.
    pub text: String,
}

/// Parameters for the `now_for_agent` agent-coordination tool.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct NowForAgentParams {
    /// Agent identifier (e.g. "claude-feature-x"). Falls back to env
    /// `HOLON_AGENT_ID` when omitted. Used to filter the now-query so
    /// the agent sees only unclaimed tasks plus tasks already assigned
    /// to itself.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Maximum number of tasks to return (default 10, max 100).
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Parameters for `claim_task` — atomic best-effort task assignment.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct ClaimTaskParams {
    /// Block id of the task to claim. Bare slugs are accepted
    /// (auto-prefixed to `block:`).
    pub task_id: String,
    /// Agent identifier; falls back to env `HOLON_AGENT_ID`.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// Parameters for `add_subtask` — append a new TODO block under an existing
/// one.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct AddSubtaskParams {
    /// Parent block id (bare slug or `block:` prefixed).
    pub parent_id: String,
    /// First line of the new block's content (used as the headline).
    pub title: String,
    /// Optional body — appended to title with a newline. Use this for the
    /// task description, runbook notes, etc.
    #[serde(default)]
    pub body: Option<String>,
    /// Initial task_state. Defaults to `"TODO"`. Pass `"DOING"` to claim
    /// in the same call (you'll usually call `claim_task` separately so
    /// the worktree/agent metadata is set).
    #[serde(default)]
    pub task_state: Option<String>,
    /// Gate for now-query visibility. Defaults to the parent's gate; if
    /// the parent has none, falls back to `"G1"`.
    #[serde(default)]
    pub gate: Option<String>,
    /// Tags to attach (e.g. `["agent"]` so the new task surfaces in
    /// `now_for_agent` immediately). NOT yet wired — currently ignored
    /// because `block.create` partitioning needs additional plumbing
    /// for edge fields. Track in a follow-up.
    #[serde(default)]
    pub tags: Option<Vec<String>>,
    /// Initial blocker list (`requires` edge field). Same caveat as `tags`.
    #[serde(default)]
    pub requires: Option<Vec<String>>,
    /// Additional `properties` key/values merged in (priority, effort, etc.).
    #[serde(default)]
    pub properties: HashMap<String, serde_json::Value>,
    /// Explicit id for the new block. When omitted, a UUID is minted.
    #[serde(default)]
    pub id: Option<String>,
}

/// Parameters for `complete_task` — marks DONE + writes a devlog file.
#[derive(Serialize, Deserialize, JsonSchema)]
pub struct CompleteTaskParams {
    /// Block id of the task to mark complete. Bare slugs accepted.
    pub task_id: String,
    /// One- to three-paragraph summary of what shipped, written to the devlog.
    pub summary: String,
    /// Agent identifier; falls back to env `HOLON_AGENT_ID`.
    #[serde(default)]
    pub agent_id: Option<String>,
    /// Optional commit SHA to record in the devlog header.
    #[serde(default)]
    pub commit_sha: Option<String>,
}

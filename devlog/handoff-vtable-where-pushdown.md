# Handoff: VTable WHERE Pushdown for Dynamic URI Params

## Problem

Holon's MCP vtables (foreign data wrappers) resolve URI template params **once at table creation time** from static config values. This means `cc_message` with `list_resource: "claude-history://sessions/{session_id}/messages"` requires a hardcoded `session_id` in the YAML config.

For a chat view, we need to query messages for whichever session the user selects — the `session_id` should come from a SQL `WHERE` clause, not the static config.

## Current Architecture

### VTable creation (`mcp_vtable.rs:200-215`)
```rust
// URI is expanded ONCE at creation time:
let uri = expand_uri_template(resource, &vtable_config.uri_params)
    .unwrap_or_else(|_| resource.clone());
FetchMode::Resource { uri }
```

### URI expansion (`mcp_sync_strategy.rs`)
```rust
pub fn expand_uri_template(template: &str, params: &HashMap<String, String>) -> Result<String>
```
Simple `{key}` replacement. Fails if any param is unresolved.

### Turso vtable bridge (`mcp_vtable.rs`)
The vtable implements Turso's virtual table interface. When SQLite executes a query against `cc_message`, Turso calls `xFilter` on the vtable module. Currently, `xFilter` fetches ALL rows from the resolved URI regardless of the WHERE clause.

## Desired Behavior

```sql
SELECT * FROM cc_message WHERE session_id = '809ab486-...'
```

Should:
1. SQLite passes the WHERE constraint `session_id = '809ab486-...'` to `xFilter`
2. The vtable recognizes `session_id` as a URI template param
3. Re-expands the URI template with the pushed-down value: `claude-history://sessions/809ab486-.../messages`
4. Fetches only that session's messages from the MCP server

## Infrastructure Already In Place

The FDW already supports filter pushdown for **tool-based** vtables (`FetchMode::Tool`):
- `key_columns` declares which columns support pushed constraints
- `column_to_param` maps column indices → MCP param names
- `filter()` receives `&[PushedConstraint]` and passes them to tool calls
- `filter_mapping` in YAML config controls which columns are pushable

For **resource-based** vtables (`FetchMode::Resource`), `filter()` ignores constraints — it just fetches the static URI (line 466 of `mcp_vtable.rs`).

**No Turso changes needed.** The pushdown plumbing (xBestIndex → PushedConstraint → filter()) is handled by Turso's existing FDW protocol.

## Implementation (all in `mcp_vtable.rs`)

### 1. Add `FetchMode::ResourceTemplate`
```rust
enum FetchMode {
    Resource { uri: String },                    // existing: static URI
    ResourceTemplate {                            // new: dynamic URI
        template: String,                         // e.g. "claude-history://sessions/{session_id}/messages"
        default_params: HashMap<String, String>,  // static params from config
    },
    Tool { ... },                                 // existing
}
```

### 2. In `McpForeignDataWrapper::new()` (~line 200)
When building FetchMode from a `list_resource`, check if any `uri_params` have empty values. If so, use `ResourceTemplate` instead of `Resource`:
```rust
let has_dynamic_params = vtable_config.uri_params.values().any(|v| v.is_empty());
if has_dynamic_params {
    FetchMode::ResourceTemplate {
        template: resource.clone(),
        default_params: vtable_config.uri_params.clone(),
    }
} else {
    let uri = expand_uri_template(resource, &vtable_config.uri_params)?;
    FetchMode::Resource { uri }
}
```

Also register `key_columns` for the dynamic param columns (same pattern as tool-based pushdown — use `filter_mapping` or auto-derive from empty uri_params).

### 3. In `McpCursor::filter()` (~line 460)
Add the new variant:
```rust
FetchMode::ResourceTemplate { template, default_params } => {
    let mut params = default_params.clone();
    for c in constraints {
        if let Some(param_name) = self.column_to_param.get(&c.column_index) {
            if let Value::Text(ref t) = c.value {
                params.insert(param_name.clone(), t.text.clone());
            }
        }
    }
    let uri = expand_uri_template(template, &params)
        .map_err(|e| LimboError::ExtensionError(format!("URI param missing: {e}")))?;
    self.fetch_via_resource(&uri)?
}
```

### 4. Auto-register key_columns for dynamic params
In `McpForeignDataWrapper::new()`, for each empty `uri_params` entry, find the matching column index and register it as a `KeyColumn` with `ConstraintOp::Eq`. Also add to `column_to_param`. This reuses the existing infrastructure that tool-based vtables already use.

## Config (no changes needed)
```yaml
  message:
    vtable:
      list_resource: "claude-history://sessions/{session_id}/messages"
      uri_params:
        session_id: ""  # empty = must come from WHERE pushdown
```
Empty value → dynamic. Non-empty value → static (baked into URI at creation).

## Key Files
- `crates/holon-mcp-client/src/mcp_vtable.rs` — **only file that needs changes**
  - `FetchMode` enum (~line 74)
  - `McpForeignDataWrapper::new()` (~line 191)
  - `McpCursor::filter()` (~line 460)
- `crates/holon-mcp-client/src/mcp_sync_strategy.rs` — `expand_uri_template()` (reuse as-is)

## Testing
1. Configure `cc_message` vtable with `session_id: ""`
2. `SELECT * FROM cc_message WHERE session_id = 'xxx'` → fetches that session's messages
3. `SELECT * FROM cc_message` without WHERE → error (unresolved template param)
4. PRQL: `from cc_message | filter session_id == $context_id` → works with context params

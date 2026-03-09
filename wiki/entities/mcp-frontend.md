---
title: MCP Frontend (AI tool interface)
type: entity
tags: [frontend, mcp, ai, tools]
created: 2026-04-13
updated: 2026-04-13
related_files:
  - frontends/mcp/src/main.rs
  - frontends/mcp/src/server.rs
  - frontends/mcp/src/tools.rs
  - frontends/mcp/src/resources.rs
  - frontends/mcp/src/di.rs
---

# MCP Frontend

The **MCP (Model Context Protocol) server** frontend. Exposes the full Holon query surface, UI inspection, undo/redo, navigation, and org file rendering to AI agents. Every other frontend also auto-launches an MCP server.

## Server

`frontends/mcp/src/server.rs` — `HolonMcpServer` implements the MCP server over stdio or HTTP. Uses `rmcp` crate for MCP protocol handling.

## Tools

`frontends/mcp/src/tools.rs` — all MCP tools are registered here. Key tools:

| Tool | Description |
|------|-------------|
| `execute_query` | Execute PRQL, SQL, or GQL query with optional `context_id` |
| `watch_query` | Subscribe to a live query stream |
| `get_display_tree` | Get the current `ViewModel` tree as JSON or text |
| `dispatch_operation` | Execute a named operation (indent, move, set_field, etc.) |
| `undo` / `redo` | Undo/redo last operation |
| `list_tables` | Inspect database schema |
| `create_table` / `drop_table` | Dynamic table management |
| `render_org` | Render a block tree as org-mode text |
| `navigate` | Move navigation cursor |

### Context Parameters

For queries involving virtual tables (`from children`, `from siblings`, `from descendants`), pass:
- `context_id` (top-level param) — resolves to `QueryContext::for_block()`
- `context_parent_id` — for sibling queries

`extract_context_from_params()` builds `QueryContext` from the params map. These are the same context params used by PRQL stdlib in `BackendEngine`. See `crates/holon/src/api/backend_engine.rs`.

## Resources

`frontends/mcp/src/resources.rs` — exposes Holon data as MCP resources (read-only subscription endpoints). Resources include the database schema and live block tree.

## HolonService Delegation

Tools delegate to `HolonService` (`crates/holon/src/api/holon_service.rs`) rather than calling `BackendEngine` directly. This ensures MCP code paths share test coverage with PBTs.

## Auto-Launch

Every frontend (GPUI, Flutter, TUI) auto-launches the MCP server as a side channel. This lets AI agents inspect live app state during a session. The server address is announced via stdout on startup.

## Related Pages

- [[entities/holon-crate]] — `HolonService` and `BackendEngine`
- [[concepts/query-pipeline]] — PRQL/SQL/GQL compilation
- [[entities/holon-frontend]] — `ViewModel` used for display tree

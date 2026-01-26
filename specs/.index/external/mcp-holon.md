---
name: holon MCP server
description: MCP server exposing the holon backend as a tool/resource provider to AI agents
type: reference
source_type: mcp
source_id: frontends/mcp/src/
fetch_timestamp: 2026-04-23
---

## Holon MCP Server

**Crate**: `frontends/mcp` (`holon-mcp`)
**Transport**: stdio or HTTP (configurable via `mcp-proxy.yaml`)

### Tool Categories

#### Database & Schema
| Tool | Description |
|------|-------------|
| `create_table` | Create a new Turso table |
| `insert_data` | Insert rows |
| `create_entity_type` | Register a typed entity |
| `drop_table` | Drop a table |
| `execute_query` | Unified SQL/PRQL/GQL query runner; reads `context_id`/`context_parent_id` as top-level params |
| `execute_raw_sql` | Raw SQL passthrough |
| `compile_query` | Compile PRQL/GQL to SQL without executing |
| `list_tables` | List all tables |

#### Reactive Streaming
| Tool | Description |
|------|-------------|
| `watch_query` | Subscribe to a live IVM query; returns watch handle |
| `poll_changes` | Poll pending CDC diffs from a watch handle |
| `stop_watch` | Unsubscribe from a watch |

#### Operations
| Tool | Description |
|------|-------------|
| `execute_operation` | Run a registered operation (create block, move, etc.) |
| `list_operations` | List available operations |
| `undo` / `redo` | Undo or redo last operation |
| `can_undo` / `can_redo` | Check undo/redo availability |

#### Debugging & Inspection
| Tool | Description |
|------|-------------|
| `list_commands` | List registered MCP commands |
| `execute_command` | Run an MCP command |
| `rank_tasks` | Rank tasks by priority |
| `list_loro_documents` | List Loro CRDT documents |
| `inspect_loro_blocks` | Inspect block content in Loro |
| `diff_loro_sql` | Diff Loro state vs SQL storage |

#### Org-Mode
| Tool | Description |
|------|-------------|
| `read_org_file` | Read and parse an org file |
| `render_org_from_blocks` | Serialize blocks back to org format |

#### UI & Navigation
| Tool | Description |
|------|-------------|
| `describe_ui` | Describe current UI state as WidgetSpec tree |
| `screenshot` | Capture UI screenshot |
| `describe_navigation` | Describe current navigation cursor |
| `send_navigation` | Programmatically navigate |

### Resources

| URI Scheme | Content |
|------------|---------|
| `holon://operations` | Metadata about registered operations |

### Context Parameters

- `context_id` — current block/entity ID (top-level param for `execute_query`)
- `context_parent_id` — parent block ID
- PRQL virtual tables `from children`, `from siblings`, `from descendants` require these params

### Keywords
mcp, tools, holon, sql, prql, reactive, operations, undo-redo, loro, orgmode

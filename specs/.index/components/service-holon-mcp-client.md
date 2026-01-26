---
name: holon-mcp-client
description: MCP client integration: resource discovery, sync engine, OAuth credentials, virtual table wrapper
type: reference
source_type: component
source_id: crates/holon-mcp-client/src/
category: service
fetch_timestamp: 2026-04-23
---

## holon-mcp-client (crates/holon-mcp-client)

**Purpose**: Connects holon to external MCP servers as data sources. Discovers resources, maps schemas, syncs data, and wraps them as Turso virtual tables (FDW).

### Key Modules & Types

| Module | Key Types |
|--------|-----------|
| `mcp_integration` | `McpIntegration` — connection + transport |
| `integration_config` | `McpIntegrationConfig`, `AuthMode`, `McpTransport` |
| `mcp_sync_engine` | `McpSyncEngine`, `ResourceSync`, `FetchResult` |
| `mcp_sync_strategy` | Sync strategy impls (full, incremental) |
| `mcp_provider` | `McpOperationProvider` |
| `mcp_notification_handler` | `McpNotificationHandler` — resource update streaming |
| `mcp_resource_discovery` | Resource template parsing |
| `mcp_schema_mapping` | Schema mapping utilities |
| `mcp_vtable` | Virtual table wrapper (FDW layer) |
| `mcp_sidecar` | Sidecar process management |
| `credential_store` | OAuth credential management |

### FDW Architecture

MCP resources become Turso virtual tables via `mcp_vtable`. This enables lazy loading of MCP data (e.g., session messages) only when the block tree is expanded — driven by `UiState.expand_state`.

### Related

- **holon**: wired into the `storage` module as an optional datasource
- **frontends/mcp**: the outgoing MCP server (inverse direction)
- **holon-core**: `DataSource` trait implemented here

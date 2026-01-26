---
name: frontend-mcp
description: Holon MCP server — exposes backend tools and resources to AI agents via stdio/HTTP
type: reference
source_type: component
source_id: frontends/mcp/src/
category: service
fetch_timestamp: 2026-04-23
---

## frontends/mcp (holon-mcp)

**Purpose**: MCP (Model Context Protocol) server that exposes the entire holon backend as a tool/resource provider. Used by AI agents (e.g., Claude) to inspect, query, and mutate the knowledge graph.

### Key Modules

| Module | Role |
|--------|------|
| `tools` | All 30+ MCP tool definitions |
| `resources` | Resource listing and reading (`holon://operations`) |
| `server` | MCP server initialization and transport |
| `di` | FluxDI wiring for MCP session |
| `types` | Shared types |
| `browser_relay` | Browser interaction relay (screenshot, navigation) |
| `telemetry` | OpenTelemetry tracing integration |

### See Also

`specs/.index/external/mcp-holon.md` — full tool reference with all tool names and parameters.

### Transport Modes

- **stdio** — default for Claude Desktop / Claude Code integration
- **HTTP** — configured via `mcp-proxy.yaml` for remote access

### Related

- **holon** `BackendEngine`: all tools delegate to the backend engine
- **holon-mcp-client**: inverse — connects holon TO external MCP servers

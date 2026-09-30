---
id: 2026-09-30-the-tui-serves-mcp-on-8520-with-mcp-disabled
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `holon-tui --mcp-enabled false` still starts the embedded MCP server, on
  port 8520 unless MCP_SERVER_PORT is set.
---

## Bug
Found by an agent (dogfooding phase 1, Increment 1d) while it wrote the TUI
stop-signal test `frontends/tui/tests/stop_signals_quit_through_the_session_shutdown.rs`.
The first version started `holon-tui` with `--mcp-enabled false`, as the GUI
test does. The TUI log said `MCP server started on http://127.0.0.1:8520`, so
every test instance bound the port of the developer's own live app (or failed
to, when that app holds it).

## Root cause
`TuiModule::configure` (`frontends/tui/src/di.rs:66-70`) registers the MCP
server unconditionally, on `MCP_SERVER_PORT` or 8520. It never reads
`HolonConfig::mcp_enabled()` (`crates/holon-frontend/src/config.rs:580`).
`GpuiModule::configure_mcp` (`frontends/gpui/src/di.rs:71-84`) reads it and
registers nothing when it is false.

## Missing piece
No test boots the TUI composition with `mcp.enabled = false` and checks that
no MCP server starts. The GUI has that guard; the TUI composition was never
drawn against it.

## Remedy
`TuiModule` registers, starts and stops the MCP server only when
`mcp_enabled()` (`frontends/tui/src/di.rs`), as `GpuiModule` does, and logs
that MCP is off. `frontends/tui/tests/boot_refusals.rs::mcp_disabled_serves_no_mcp`
boots `holon-tui --mcp-enabled false` on its own `MCP_SERVER_PORT` and finds no
listener; it was red (the port accepted a connection) before the gate.

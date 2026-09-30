---
id: 2026-10-01-an-unparsable-mcp-server-port-silently-falls-back-to-8520
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  An unparsable MCP_SERVER_PORT makes holon-tui serve MCP on 8520 without a
  word.
---

## Bug
Found by the implementer of the dogfood follow-ups lane (code audit,
`lane-logs/dffu-r2-report.md`, Findings).

## Root cause
`frontends/tui/src/di.rs:81-84` reads the variable with `.ok()`,
`.parse().ok()` and `.unwrap_or(8520)`, so a typo binds the default port,
which another instance may already own. `frontends/mcp/src/di.rs:533` reads
the same variable; not checked whether it has the same shape.

## Missing piece
No test sets an invalid `MCP_SERVER_PORT`.

## Remedy
Open, not fixed.

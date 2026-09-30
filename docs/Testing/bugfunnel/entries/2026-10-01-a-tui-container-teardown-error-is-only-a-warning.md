---
id: 2026-10-01-a-tui-container-teardown-error-is-only-a-warning
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A failed or timed-out container teardown in holon-tui (including the MCP
  server stop) is logged as WARN and never reaches the exit status.
---

## Bug
Found by the implementer of the dogfood follow-ups lane (code audit,
`lane-logs/dffu-r2-report.md`, Findings).

## Root cause
`shut_down` in `frontends/tui/src/main.rs:167-168` logs
`app.shutdown()` errors and its 10 s timeout with `tracing::warn!` and
returns only the session shutdown's result.

## Missing piece
No test makes the container teardown fail.

## Remedy
Open, not fixed.

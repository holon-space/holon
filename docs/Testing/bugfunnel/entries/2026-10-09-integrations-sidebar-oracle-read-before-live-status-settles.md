---
id: 2026-10-09-integrations-sidebar-oracle-read-before-live-status-settles
date: 2026-10-09
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  `integration_rows_show_name_icon_aligned_status_and_open_their_view` wrote
  status words over the fake MCP peer's row and read its oracle before the
  window opened, while the connect registry was still writing that row; the row
  then painted the newer `Connected` glyph against a stale `Pending` oracle.
---

## Class

`FALSE-ALARM`: no product defect. The sidebar row painted the status the mirror
held at paint time (`Connected` → "●"). The test compared it with a snapshot of
the mirror taken earlier, before the registry's last write.

## Bug

`holon-gpui --test integrations_sidebar_rows_windowed
integration_rows_show_name_icon_aligned_status_and_open_their_view` failed 3/3
on main `73e73daa` and on integration at
`frontends/gpui/tests/integrations_sidebar_rows_windowed.rs:249` ("fake-mcp is
"Pending" in the mirror, so its row must paint "◐" — it painted "●""). Found by
the orchestrator's windowed-test triage (lane gpui-reds); there was no
known-red row.

## Root cause

Commit `2fbbc6752e75` (2026-10-07, D109 Inc 5) made the composed boot connect a
fake MCP peer through the real `McpIntegrationsModule`. The registry owns that
row's `status` column: the sync-health pump writes each verdict
(`crates/holon-app/src/mcp_integrations.rs:121`, `record_status` →
`set_integration_status`, `crates/holon-app/src/integration_projection.rs:280`).
A table dump in the test (log `lane-logs/gpui-reds/isr-instrumented-1.log`)
showed the sequence: fake-mcp is `Syncing` when the test switches every row on,
the test's status spread overwrites it with `Pending`, the test reads the
oracle, and after the window settles the pump has written `Connected`. The
other providers are not connected, so nothing writes their rows after the
spread.

## Missing piece

The oracle was a pre-launch snapshot of a table that a live writer still
changes. Also, no gate runs the windowed GPUI tests, so the boot change landed
green.

## Remedy

The test waits until the fake peer's row reads `Connected`, spreads the status
words only over the rows no registry writes, and reads the oracle after the
window settles (`frontends/gpui/tests/integrations_sidebar_rows_windowed.rs:222,458,500`).
Teeth: with the status shadow builder painting "◐" for every word, the test
goes red ("ics-calendar is "Unavailable" in the mirror, so its row must paint
"○" — it painted "◐"").

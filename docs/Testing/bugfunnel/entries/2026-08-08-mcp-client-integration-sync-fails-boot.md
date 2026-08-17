---
id: 2026-08-08-mcp-client-integration-sync-fails-boot
date: 2026-08-08
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  MCP client integration sync fails at boot and then on every poll, forever,
  with an unactionable error
source_line: 767
---

## Bug

(dogfood-explorer gate pass) **MCP client integration sync fails at boot and
then on every poll, forever, with an unactionable error**: `initial sync
failed error=Batch transaction failed: Database error: Failed to execute
statement: datatype mismatch`, then `poll_resync{entity=jp_posts}` repeating
the same every ~60s with no backoff, no recovery and no column/row context.

## Root cause

dogfood-explorer gate pass — **the MCP client integration sync fails at boot
and then on every poll, forever, with an unactionable error**. With
`docs/integrations/*.yaml` copied into `{config_dir}/integrations` as the
loader requires: `initial sync failed error=Batch transaction failed:
Database error: Failed to execute statement: datatype mismatch`, then
`poll_resync{entity=jp_posts}: poll resync failed error=…datatype mismatch`
repeating every ~60s for the life of the process, with no backoff, no
recovery and no column/row context. Disclosed rather than silent (good) but
permanently degraded, and the repetition drowns the log. ENVIRONMENT: the
failing path is the live MCP-client → Turso batch write, which the headless
keystone never wires — no integration sidecar is loaded in the test
environment at all. Related cosmetic symptom on the same feature: after a
restart the sidebar "Integrations" section paints one BLANK bullet while
`SELECT provider_name, updated_at FROM sync_states` returns `orgmode |
2026-08-08 08:05:03`, which the same section rendered as text before the
restart. Evidence:
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§4)

## Missing piece

The failing path is the live MCP-client → Turso batch write; the headless
keystone loads no integration sidecar at all, so the path does not exist in
the test environment.

## Remedy

**OPEN — reported, not fixed.** Related cosmetic symptom: after a restart
the sidebar "Integrations" section paints one BLANK bullet while
`sync_states` returns `orgmode \ | 2026-08-08 08:05:03` (it rendered as text
before the restart). Evidence
`docs/Testing/fixture-logs-2026-08-08/dogfood-blank-link-unrepresentable-and-misc.txt`
§4.

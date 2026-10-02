---
id: 2026-10-02-dense-patch-rolls-back-a-correct-patch-under-projection-lag
date: 2026-10-02
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  When the Loro→SQL projection lags more than READBACK_WINDOW (2 s) behind a
  dense_patch, the read-back gives up, reports the CORRECTLY applied rows as
  mismatched, rolls the whole patch back and returns an error to the agent.
---

## Bug

Found by analysis, not by a test: the DD tracer lane (decisions D26.b / D27.a,
plan `lane-logs/dd-tracer-plan.md` §12.1) chose `dense_patch`'s read-back as
the first session reader and read it to find the wait it replaces.

## Root cause

- `apply_plan` (`frontends/mcp/src/tools.rs:561`) dispatches every op, then
  calls `read_back` (`:612`). A non-empty mismatch list ROLLS THE BATCH BACK
  (`:619-631`) and returns `readback_error` (`:632`).
- `read_back` (`:757-861`) polls `BLOCK_READ_TABLE` every 25 ms (`:859`) until
  the rows match or `READBACK_WINDOW` = 2 s (`:752`, `:781`) has passed; at the
  deadline it returns the rows that do not match YET (`:849-850`).
- `BLOCK_READ_TABLE` is fed by the Loro→SQL projection, which runs after the
  write returns. A projection pass that lags more than 2 s (a large import, a
  slow machine, the projector lag hook) therefore turns a patch the authority
  holds exactly as the text said into a "mismatch": the agent sees an error and
  the patch is taken back. Presence of the rows within a fixed window is not
  the version of the write the read needs.

## Missing piece

The keystone's dense_patch transitions run with no projection lag, and no
lag bin in `just projector-lag-lock` drives a dense_patch, so the window is
never exceeded under test.

## Remedy

OPEN. Red test: the planned `dense_patch_under_lag` bin in
`just projector-lag-lock` (`HOLON_TEST_PROJECTOR_LAG_MS=3000`, a dense_patch
that edits one row and moves one row; today the read-back times out and the
patch is rolled back). Fix: DD tracer Inc 4 — the read-back waits through the
MCP connection's session until the view engine's frontier passes the patch's
write ticket, then compares once (bounded wait, loud error after 30 s).

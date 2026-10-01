---
id: 2026-10-01-mcp-undo-pops-the-humans-undo-stack
date: 2026-10-01
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  The MCP `undo` tool calls the one shared undo stack, which holds only human
  entries because agent ops push none, so an agent's `undo` reverts the human's
  last gesture.
---

## Bug
Found by reading, not reproduced: the read-only spike SP6 (human-agent
contention table, section 5.1, `scoped-net/sp6-human-agent.md` in the session
scratchpad). Related but different from
`2026-07-21-mcp-undo-tool-reports-undone-successfully`, which is about the
success message on a no-op; this entry is about WHOSE entry gets undone.

## Root cause
By reading, not reproduced. `OpOrigin::Agent` ops never push undo entries; only
`OpOrigin::User` does (`crates/holon-api/src/operation_engine.rs:35-60`, doc on
`Agent` says agent actions are reverted through the supervision surface). The
MCP `undo` tool (`frontends/mcp/src/tools.rs:1641`) still calls
`HolonService::undo`, which reaches the shared `op_engine.undo()`. When agent
and GPUI user share one engine, the stack holds only the human's entries. `redo`
has the same shape.

## Missing piece
No keystone transition draws an agent-origin `undo`/`redo` next to human edits,
and no invariant says "an agent undo never consumes a human entry".

## Remedy
OPEN. Agent `undo` must either refuse (point at the supervision revert) or act
on an agent-owned stack, never on the human's. Add a keystone transition
"agent undo after a human edit" that goes red first.

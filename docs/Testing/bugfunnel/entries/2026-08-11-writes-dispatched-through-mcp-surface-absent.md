---
id: 2026-08-11-writes-dispatched-through-mcp-surface-absent
date: 2026-08-11
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  Writes dispatched through the MCP `execute_operation` surface are absent
  from the undo stack, so a following `undo` reports success while silently
  reverting an OLDER, unrelated user operation.
source_line: 737
---

## Bug

(task #92 Cucumber-dogfood rehearsal, found by DOGFOODING at main
`644a399d`; no automated test produced it) **Writes dispatched through the
MCP `execute_operation` surface are absent from the undo stack, so a
following `undo` reports success while silently reverting an OLDER,
unrelated user operation.** Reproduced twice. Cleanest run: MCP
`cycle_task_state` set `task_state` TODO on block A; `undo` returned
`{"success":true,"message":"Operation undone successfully"}`, left A's
`task_state` TODO untouched, and instead reverted A's CONTENT from
`EXTERNALZZZZ` to `EXTERNAL` — an edit from several gestures earlier.
`can_undo` said `{"available":true}` throughout. Boundary controls: the SAME
cycle through the production keybinding (`cmd+enter`) IS undoable and undid
correctly; and an MCP `set_field` followed by `undo` failed LOUD (`undo:
dropped stale entry (state changed under undo: <id>.content expected
String("EXTERNALZZZZ") but found Some(String("EXTERNAL")))`, ERROR-logged).
The guard exists and works — the defect is that when no drift is detectable,
the wrong-op undo succeeds silently.

## Root cause

task #92 Cucumber-dogfood rehearsal, found by DOGFOODING at main `644a399d`:
**writes dispatched through the MCP `execute_operation` surface are absent
from the undo stack, so a following `undo` reports
`{"success":true,"message":"Operation undone successfully"}` while silently
reverting an OLDER, unrelated user operation.** Reproduced twice. Cleanest
run: `cycle_task_state` via MCP set `task_state` TODO on block A; `undo`
then returned success, left A's `task_state` TODO untouched, and instead
reverted A's CONTENT from `EXTERNALZZZZ` back to `EXTERNAL` — an edit from
several gestures earlier. `can_undo` reported `{"available":true}`
throughout. CONTRAST that establishes the boundary and shows the engine is
half-right: the same cycle performed through the REAL production keybinding
(`cmd+enter`, `send_key_chord`) IS undoable and undid correctly; and a later
MCP `set_field` followed by `undo` failed LOUD with a precise drift message
(`undo: dropped stale entry (state changed under undo: <id>.content expected
String("EXTERNALZZZZ") but found Some(String("EXTERNAL")))`, logged at
ERROR). So the guard exists and works — the defect is that when no drift is
detectable the wrong-op undo succeeds silently. Primary ENVIRONMENT: the MCP
dispatch path is a first-class surface (CLAUDE.md: every frontend launches
one, agents drive it) but no rung exercises undo AFTER an MCP-origin write;
the keystone drives transitions through the UI-origin path only, so the
origin axis that decides undo-stack membership is absent from the test
environment. Secondary COVERAGE: no draw interleaves an MCP-origin write
with `UndoLastMutation`. Consequence beyond the app: an agent or a dogfood
session cannot trust `undo` after its own `execute_operation` calls — it may
destroy the user's earlier work. Missing piece: either record MCP-origin ops
on the undo stack, or make `can_undo`/`undo` report "nothing of yours to
undo" rather than popping someone else's op. OPEN.)

## Missing piece

ENVIRONMENT: the MCP dispatch path is a first-class surface (CLAUDE.md —
every frontend launches one; agents drive it) but no rung exercises undo
AFTER an MCP-origin write, so the origin axis that decides undo-stack
membership is absent from the test environment. COVERAGE (secondary): no
draw interleaves an MCP-origin write with `UndoLastMutation`. Missing piece:
either record MCP-origin ops on the undo stack, or have `can_undo`/`undo`
report "nothing of yours to undo" instead of popping someone else's op.

## Remedy

OPEN — reported, not fixed. Operational consequence beyond the app: an agent
or dogfood session cannot trust `undo` after its own `execute_operation`
calls.

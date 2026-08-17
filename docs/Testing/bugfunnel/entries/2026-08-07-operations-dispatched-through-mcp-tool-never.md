---
id: 2026-08-07-operations-dispatched-through-mcp-tool-never
date: 2026-08-07
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Operations dispatched through the MCP `execute_operation` tool never enter
  the in-memory undo stack, so an agent's destructive edits are not
  user-undoable.
source_line: 1173
---

## Bug

(overnight dogfood-explorer, same session — flagged for a RULING, not
asserted as a defect) **Operations dispatched through the MCP
`execute_operation` tool never enter the in-memory undo stack, so an agent's
destructive edits are not user-undoable.** `delete_subtree` on `block:d-l3a`
removed a 4-block, 3-level subtree from SQL and wrote the deletion through
to `Deep.org`, after which `can_undo` answered `{"available":false}` and
`undo` answered `{"success":false,"message":"Nothing to undo"}`; `set_field`
and `delete_keep_children` through the same tool behave identically, which
is what rules out "delete_subtree specifically is not undoable". Mechanism
confirmed by inspection: the stack is pushed only for `OpOrigin::User`
(`crates/holon/src/api/operation_engine.rs:833,1120,1765`) while the MCP
path dispatches as `OpOrigin::Agent`, and `can_undo` reads the in-memory
stack only (`:1908`). The op IS still recorded durably in
`OperationLogStore`, so the information to undo exists — it is simply not
reachable from the UI affordance.

## Root cause

overnight dogfood — operations dispatched through the MCP
`execute_operation` tool never enter the in-memory undo stack, so an agent's
destructive edits are NOT user-undoable. `delete_subtree` on a 4-block,
3-level subtree removed it from SQL AND wrote the deletion through to the
org file, after which `can_undo` answered `{"available":false}` and `undo`
answered "Nothing to undo"; `set_field` and `delete_keep_children` behave
identically. Mechanism confirmed by inspection: the stack is pushed only for
`OpOrigin::User` (`crates/holon/src/api/operation_engine.rs:833,1120,1765`)
while the MCP path dispatches as `OpOrigin::Agent`, and `can_undo` reads the
in-memory stack only (`:1908`); the op IS still recorded in the durable
OperationLogStore. Possibly intended — flagged for a ruling rather than
asserted as a defect — but the user-facing consequence is that an agent can
cascade-delete vault content with no undo affordance)

## Missing piece

Plausibly the intended design (agent edits deliberately kept off the user's
undo stack), which is why this is filed for a ruling rather than as a
defect. The user-facing consequence is the part that needs deciding: an MCP
agent can cascade-delete vault content and leave the user with no undo
affordance at all. Missing piece if it IS to be changed = either push
Agent-origin ops onto the same stack, or surface an explicit "revert agent
operation" path over the OperationLogStore that already holds them.

## Remedy

OPEN 2026-08-07 — needs Martin's ruling. Positive companion result from the
same probe: `delete_subtree` at depth 3 now works correctly end to end (all
four descendants removed from SQL and from the org file), i.e. the 5ebe8208
fix holds under dogfooding.

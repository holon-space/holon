---
id: 2026-07-10-gpui-user-ever-reaches-undo-stack
date: 2026-07-10
gap: ENVIRONMENT
secondary: COVERAGE
status: OPEN
summary: >-
  GPUI: no user op ever reaches the undo stack (can_undo=false after
  create/split/join) and cmd+z is unbound ("No handler matched the key chord")
  — undo completely non-functional in the desktop app
source_line: 881
---

## Bug

GPUI: no user op ever reaches the undo stack (can_undo=false after
create/split/join) and cmd+z is unbound ("No handler matched the key chord")
— undo completely non-functional in the desktop app

## Missing piece

MECHANISM CORRECTED (research 2026-07-10): there is NO bypass —
keystroke→intent-chain→FrontendSession→BackendEngine→DispatchingOperationEngine
is correctly wired and pushes any `UndoAction::Undo` it receives (GPUI and
MCP share ONE stack via the same injector). The gap is PROVIDER COVERAGE:
`create`/`delete`/`set_field`/`split_block`/`join_block`/`cycle_task_state`
all hard-return irreversible (split has a never-done TODO) while
`indent`/`outdent`/`move_block` return real inverses — this also explains
the 2026-07-07 "undo skipped join, undid older outdent". cmd+z is simply
unbound (`FrontendSession::undo/redo` exist as call targets)

## Remedy

open — RULING (Martin 2026-07-10): Option A shaped for C — inverse coverage
at the providers, entries as serializable data (`Operation` already
Serde-able) migratable to the ADR 0024 substrate (ADR L241 endorses "undo =
reversed arcs", Phase 3 plans `fired-by` provenance); effect retraction OUT
of v1; provenance must land BEFORE `create` gets an inverse (action_watcher
shares the stack — rule-fired creates would pollute user undo);
missing-undo-classification = loud error; keystone rung = `undo∘op ≡
identity` vs ref | UNDO v1 LANDED overnight (2026-07-11, U1+U2+U3): OpOrigin
provenance (rule/sync/ingest never on stack), typed
Undo/DeclaredIrreversible classification (Undeclared = loud error), C-shaped
persistent entries (`undo_log` per replica DB, precondition fingerprints,
stale → loud StaleDropped), word-boundary grouping (RULED: non-alnum closes
group), real inverses for set_field/cycle_task_state/create/delete-leaf
(subtree delete = DeclaredIrreversible pending wave 2), cmd+z/cmd+shift+z
bound (3-layer: context-free actions + global fallback + capture-phase
intercept of editor-local undo) with StaleDropped toast. OPEN: U4 split/join
compound inverses; U5 keystone `undo∘op≡identity` rung; keystone
`undo_last_mutation` model may need updating now that more ops are undoable

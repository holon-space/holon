---
id: 2026-07-21-cycle-task-state-undo-stack-undo
date: 2026-07-21
gap: ORACLE
secondary: COVERAGE
status: NOTED
summary: >-
  cycle_task_state not on the undo stack — undo reports "undone successfully"
  but reverts the PREVIOUS op; task state never revertible via undo
  (User-relevant class re-confirmed live).
source_line: 1058
---

## Bug

cycle_task_state not on the undo stack — undo reports "undone successfully"
but reverts the PREVIOUS op; task state never revertible via undo
(User-relevant class re-confirmed live).

## Missing piece

metamorphic undo invariant: for any undoable op O, state;O;undo == state;
per-op undo-coverage inventory

## Remedy

RETRIAGED+COVERED 2026-07-21 (cycle 2) — does NOT reproduce on current tree:
provider inverses + keystone toggle rung already present; round-3 sighting =
agent-origin/stale-binary ENVIRONMENT artifact. Class locked by two
red-proven tests (exact-B2 e2e via real DI prod session + metamorphic
state;O;undo==state over property-backed User ops,
undo_cycle_task_state_coverage.rs). Only delete-subtree + dismiss_advice
remain non-undoable (disclosed)

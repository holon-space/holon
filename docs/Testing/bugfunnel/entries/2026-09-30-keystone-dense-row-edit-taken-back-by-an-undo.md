---
id: 2026-09-30-keystone-dense-row-edit-taken-back-by-an-undo
date: 2026-09-30
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The keystone's reference undid an agent's dense_patch row edit along with an
  earlier user split, which prod's undo never journals, so the run went red on
  a content divergence with no product defect.
---

## Bug
Keystone `pbt general 8`, Inc 6 round 10 run 3 (`lane-logs/inc6r10-plain.log`):
`inv-blocks-match-ref` on `block:parent` (SUT `"To\nubusl\n- ibtj"`, reference
`"Gu"`) after `DenseProjectionEdit(EditFirstRow)` then `UndoLastMutation`.

## Root cause
The reference's undo restores a whole block-state snapshot
(`ReferenceState::pop_undo_to_redo`). The dense edit is an MCP write with
Agent origin, which prod's undo stack does not hold, so prod's undo reverted
only the earlier split while the reference also took back the row edit. Both
sides edited the same row: the step-9 invariants passed. Replayed
deterministically (`lane-logs/inc6r11-c-probe.log`, case `p-full-turso`).

## Missing piece
The reference had no notion of a write outside the user's undo history for
dense edits (it has one for file ingest).

## Remedy
Inc 6 round 11. `ReferenceState::carry_block_text_across_history` writes the
edited row's text and task state into every undo and redo snapshot. The driver
picks rows under the model's sibling order (`SIBLING_ORDER`, `sort_key` then
`id`). Reproducer: `dense-row-edit-survives-undo-of-an-earlier-split` in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`.
The same class for `MoveFirstChildToEnd` and `AppendChild`:
`2026-09-30-keystone-dense-move-and-append-journaled-as-undo-steps`.

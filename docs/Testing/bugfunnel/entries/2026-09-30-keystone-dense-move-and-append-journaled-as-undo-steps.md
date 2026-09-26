---
id: 2026-09-30-keystone-dense-move-and-append-journaled-as-undo-steps
date: 2026-09-30
gap: FALSE-ALARM
secondary: null
status: FIXED
summary: >-
  The keystone's reference journaled an agent's dense_patch move and append as
  undo steps, which prod never does, so an undo after either took back the
  agent write in the model and the previous user gesture in prod.
---

## Bug
Found by the Inc 6 round-11 verifier by reading the model
(`lane-logs/inc6r11v-verify.md`, item C). Not yet seen red in a generated run.
Reproduced by `dense-move-survives-undo-of-an-earlier-create` and
`dense-append-survives-undo-of-an-earlier-create`: the reference kept
`block:cr4`, prod undid its create (`lane-logs/inc6r12-ma-red.log`).

## Root cause
Same class as `2026-09-30-keystone-dense-row-edit-taken-back-by-an-undo`. The
dense tools write with Agent origin, which prod does not journal. The model's
`MoveFirstChildToEnd` called `push_undo_snapshot`, and `AppendChild` created
through `create_block_under`, which snapshots.

## Missing piece
The model had no way to carry a structural agent write across the undo
history.

## Remedy
Inc 6 round 12. Both arms write without a snapshot and call
`ReferenceState::carry_block_placement_across_history`, which puts the block
and its place among its siblings into every undo and redo snapshot that holds
its parent. Both reproducers are in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`
(green: `lane-logs/inc6r12-ma-green.log`).

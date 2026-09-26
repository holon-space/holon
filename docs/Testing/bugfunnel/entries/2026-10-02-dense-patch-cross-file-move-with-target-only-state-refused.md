---
id: 2026-10-02-dense-patch-cross-file-move-with-target-only-state-refused
date: 2026-10-02
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  dense_patch refuses a valid edit that moves a row into another file and sets
  a task state only the target file declares, when the row's old state is not in
  the target file's keyword ring; nothing is written (over-refusal, safe direction).
---

## Bug
Found by the cross-document engine property test
(`a_generated_cross_document_edit_reads_back_exactly_or_is_refused`) after its
file-side shapes were added in lane decision Inc 6 round 7c. Not fixed on
purpose: the refusal is the safe direction (0 ops landed, no store or file change).

Seed (`lane-logs/inc6r7c-engine2.log`): CrossCase ring_a 3, ring_b 0,
a_states [Some(0)], new_state Some((true, 0)).

## Root cause
The planner orders the ops Move, then SetState. The move dispatch
(`move_block`) checks the row's CURRENT state against the target document's ring
and fails ("task_state NEXT ... not a keyword of the document"), so the plan
is refused with "the 0 op(s) dispatched before it". The edit is representable:
SetState first, or a move that carries the new state, would land.

## Missing piece
The planner does not order the state change before a move whose old state the
target ring lacks. No test drew this shape until the cross test gained
`new_state` against differing rings; the cross test tolerates this refusal text.

## Remedy
OPEN. Either order SetState before the Move when the new state is in the target
ring, or predict the move failure in the plan and refuse by the row's name
before dispatch. Then remove the tolerated refusal text from the cross test.

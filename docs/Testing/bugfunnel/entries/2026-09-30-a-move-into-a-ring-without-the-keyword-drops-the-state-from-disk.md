---
id: 2026-09-30-a-move-into-a-ring-without-the-keyword-drops-the-state-from-disk
date: 2026-09-30
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A move_block (or indent, set_field parent_id, undo/redo replay) of a task into
  a file whose #+TODO: ring does not declare its keyword was accepted; the store
  kept the keyword and write-back wrote the headline without it, so the task
  state was lost from disk.
---

## Bug
Found by the Inc 6 round-10 adversarial verifier (`lane-logs/inc6r10v-verify.md`,
DEFECT A, probe `zz10v_p4`). Page A `#+TODO: TODO | CANCELLED` held
`TODO Movable`; one `move_block` put it under page B `#+TODO: NEXT | SHIPPED`.
The call succeeded, the store kept `("TODO", "active")`, and page B's file read
`** Movable`. The same edit sent as a dense_patch was refused by the planner.

## Root cause
The engine re-derived the category after a move (`recategorize`,
`crates/holon/src/api/operation_engine.rs`) with the plain constructor, not the
ring-membership check the set_field path used, and dispatched its own
`set_field(TaskStateWrite)`, which `classify_task_state` passes unchecked. No
check ran before the move itself. The dense_patch planner also judged a child
row by its old document when only its parent moved.

## Missing piece
- The generated cross-document property moved only leaf rows, and only
  counted "keyword the target ring lacks" when the edit also set a state.
- Every engine move test moved between two rings that both declared the
  keyword.

## Remedy
Inc 6 round 11. `rehomed_root` runs before every block write at the three
dispatch seams (main, compound constituent, undo/redo replay): it finds the
subtree a move, indent, outdent, parent/ring/`Page`-tag write hands to another
document or ring, and refuses by name every task block whose keyword that ring
lacks (`state_in_ring`, the same function a `set_field` of a keyword uses).
`recategorize` derives through `state_in_ring` too. The planner judges a row by
the document its moved parent lands in. Tests: four engine tests in
`crates/holon/tests/cycle_task_state_vocabulary.rs` (leaf, subtree, indent,
ring edit), and the cross-document property now draws subtrees and plain
`move_block` operations and requires a refusal whenever a moved keyword is
missing from the target ring (red with the round-10 engine:
`lane-logs/inc6r11-red-cross.log`).

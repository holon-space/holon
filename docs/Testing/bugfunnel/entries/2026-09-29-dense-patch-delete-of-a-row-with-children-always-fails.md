---
id: 2026-09-29-dense-patch-delete-of-a-row-with-children-always-fails
date: 2026-09-29
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  dense_patch `delete: [alias]` of a row that has a child always failed after
  dispatch (the bare `block.delete` refuses to cascade), named the block id
  instead of the row, although the tool promised deletion "with their
  subtrees".
---

## Bug
Found by the Inc 6 round-4 adversarial verifier
(`lane-logs/inc6r4v-verify.md`, D1, probe
`zzp_delete_a_parent_whose_child_the_text_omits`). Present on main
`d4f426ffc965`: the applier dispatched `block.delete`, which refuses a block
with children (`refusing to cascade. Use delete_subtree ...`); the batch was
rolled back and the error named `block:pp0-r0`.

## Root cause
`frontends/mcp/src/tools.rs` dispatched `"delete"` for `PatchOp::Delete`,
while the tool description promised subtree removal.

## Missing piece
The generated property seeded flat rows only, so every drawn delete was a
leaf; no real-engine rung deleted a row with a child.

## Remedy
Inc 6 round 5: `PatchOp::Delete` dispatches `delete_subtree`; a deleted
descendant of another deleted row is not planned twice; a row the text keeps
in place inside a deleted subtree refuses the patch by its row name before any
write. The real-engine property seeds nested rows and draws deletes of leaf and
non-leaf rows (with the subtree leaving the text, and with the children staying).

---
id: 2026-10-01-a-set-field-re-parent-skips-the-destination-write-tier
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  A re-parent sent as set_field("parent_id") is judged only by the moved
  block's own write tier, so a writable block can be moved under a block that a
  read-only document (cooklang) owns.
---

## Bug
Found by the lane I1 verifier (code audit, round 2 of lane I1, the one
write-tier check above the creation-slot leg branch). Not reproduced in a
running app.

## Root cause
`OperationDispatcher::enforce_write_tier`
(`crates/holon/src/api/operation_dispatcher.rs`, `for key in ["id", "parent_id"]`)
judges the `id` and `parent_id` PARAMS. A re-parent through
`set_field` carries the destination in `value` (`field: "parent_id"`), which
`crates/holon-app/src/ordered_block_crud.rs` (the `"set_field"` parent_id arm)
turns into a placement under the new parent. The destination's tier is never
asked.

## Missing piece
No keystone writer re-parents a block onto a block the read-only document
owns, so no case can reach the state; the destination is not a judged subject
of the gate.

## Remedy
OPEN. Judge `value` as a subject when `field == "parent_id"`, and add a keystone
transition (or widen an existing move) whose destination can be a read-only-homed
block, red before the fix.

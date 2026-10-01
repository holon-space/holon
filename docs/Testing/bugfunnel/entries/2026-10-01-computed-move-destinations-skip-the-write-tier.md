---
id: 2026-10-01-computed-move-destinations-skip-the-write-tier
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  indent, outdent, move_up, move_down and join compute their destination inside
  the provider, so the write-tier gate judges only the moved block and never
  the block it lands under or merges into.
---

## Bug
Found by the lane I1 verifier (code audit, round 2 of lane I1). Not reproduced
in a running app.

## Root cause
`OperationDispatcher::enforce_write_tier`
(`crates/holon/src/api/operation_dispatcher.rs`) judges the `id` and
`parent_id` params of a block op. For indent, outdent, move_up, move_down and
join the params name only the moved block; the new parent (the previous
sibling, the grandparent) or the join survivor is decided later in the
provider. A writable block next to a block that a read-only document owns can
therefore be placed under, or merged into, that block.

## Missing piece
No keystone case puts a writable block next to a read-only-homed block and
then indents, outdents, moves or joins it, so the computed destination is never
read-only in any case; the gate has no hook for a destination the provider
computes.

## Remedy
OPEN. Either resolve the destination before the gate (a typed placement the
dispatcher can judge) or judge it at the provider seam where it is computed;
pin with a keystone row whose structural op lands next to a read-only-homed
block, red before the fix.

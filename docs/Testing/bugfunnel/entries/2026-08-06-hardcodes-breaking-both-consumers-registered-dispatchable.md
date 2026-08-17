---
id: 2026-08-06-hardcodes-breaking-both-consumers-registered-dispatchable
date: 2026-08-06
gap: COVERAGE
secondary: null
status: PARTIAL
summary: >-
  `impl BlockEntity for holon_api::Block` hardcodes `depth() -> 0`, breaking
  both consumers: `delete_subtree` (a registered dispatchable op) refuses any
  subtree deeper than one level because its deepest-first sort is a no-op, and
  `move_block` writes `depth=1` on every move and shifts the subtree by a
  cumulative +1.
source_line: 770
---

## Bug

(inspection, during the task-#23 depth repair) **`impl BlockEntity for
holon_api::Block` hardcodes `depth() -> 0`, breaking both consumers:
`delete_subtree` (a registered dispatchable op) refuses any subtree deeper
than one level because its deepest-first sort is a no-op, and `move_block`
writes `depth=1` on every move and shifts the subtree by a cumulative +1.**
Root finding: `block_raw.depth` has NO authoritative writer — org ingest
omits it, the Loro projector omits it on purpose as 'derived from the tree
structure', `BlockWriteField` classes it as storage bookkeeping, and nothing
reads it (`blocks_with_paths` does not select it; the frontend and the PBT
reference both recompute depth from `parent_id`).

## Root cause

inspection during the task-#23 depth repair — ONE dead column, two escapes
(counted once, one ledger row). (i) `delete_subtree`, a REGISTERED
dispatchable op, could not delete any subtree deeper than one level: it
ordered its deletes deepest-first via `sort_by_key(Reverse(d.depth()))`
while `impl BlockEntity for holon_api::Block` hardcodes `depth() -> 0`
(holon-core/src/traits.rs:2480), so the sort was a no-op and the walk hit a
non-leaf first, where single-block `delete`'s fail-closed guard refused it
(`refusing to cascade`). FIXED: the rank is now derived from `parent_id`
WITHIN the descendant set (`subtree_ranked_deepest_first`), which needs no
stored depth at all and fails loud on a chain that leaves the set or cycles.
(ii) `move_block` derives `new_depth = parent.depth()+1` and `delta =
new_depth - block.depth()` from the same hardcode, so every SQL-backed move
writes `depth=1` and shifts its subtree by a cumulative +1 — STILL OPEN, and
deliberately NOT closed by making `depth()` read the column:
`block_raw.depth` HAS NO AUTHORITATIVE WRITER. The org ingest omits it
(`holon-orgmode/src/block_params.rs`), the Loro projector — the Phase-2 sole
writer of block columns — omits it ON PURPOSE ('derived from the tree
structure', `holon-loro/src/block_cell_registry.rs:286,293`),
`BlockWriteField::parse` rejects it as storage bookkeeping, and
`pbt_infrastructure.rs:306` recomputes depth by walking parents rather than
trusting it. Its only writers are the broken delta arithmetic itself and
`resolve_destination_chain`'s page-segment mint; `blocks_with_paths` does
not even select it and the frontend computes its own depth. A `depth()`
reading the DEFAULT-0 column would compute `0+1` and turn the
characterization test green while prod stayed identically wrong. Fork left
to Martin: make the column authoritative at ingest+backfill, vs. delete it
and derive. GAP: no test in either arm ever asserted post-move depths or
drove `delete_subtree` past one level — the op has no keystone transition at
all)

## Missing piece

No test in either arm asserted post-move depths, and nothing anywhere drove
`delete_subtree` past one level — it has no keystone transition, so neither
the interaction nor an oracle existed.

## Remedy

PARTIAL 2026-08-06: `delete_subtree` FIXED by deriving deepest-first rank
from `parent_id` within the descendant set
(`holon-core/src/traits.rs::subtree_ranked_deepest_first`), fail-loud on an
escaping or cyclic chain — correct regardless of what the column holds.
Red-first:
`delete_subtree_deletes_a_three_level_subtree_with_unwritten_depths` failed
with `block:sub-c has 1 child(ren); refusing to cascade` (the fixture stores
depth 0 on every row on purpose — the value prod actually writes).
`move_block` NOT fixed: the fix shape is a Martin ruling (make depth
authoritative at ingest + backfill, vs. delete the column and derive depth),
because reading the DEFAULT-0 column would flip the characterization test
green without changing prod behaviour by one bit. Characterization test
`indent_depth_is_wrong_because_block_entity_depth_is_hardcoded_zero` kept
and re-documented with that trap.

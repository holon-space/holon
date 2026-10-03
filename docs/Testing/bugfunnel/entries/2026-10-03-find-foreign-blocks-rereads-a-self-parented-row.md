---
id: 2026-10-03-find-foreign-blocks-rereads-a-self-parented-row
date: 2026-10-03
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  find_foreign_blocks read an asked block twice when the block is its own parent, so a file ingest
  broke its O(ids x depth) read bound.
---

## Bug

`find_foreign_blocks_cost answers_the_whole_store_question_from_the_asked_blocks` failed once in
the full core nextest run on main `03078ee3` (night triage `main-reds/full-1.log:2083`): `read 2
rows to answer 1 ids in a store of 2 (bound 1 = ids x deepest chain)`. The shrunk input is one
self-parented block. It passed 0/10 times alone, because the generator rarely draws a
self-parent. Found by the reds triage of the night of 2026-10-03.

## Root cause

`find_foreign_blocks` (`crates/holon-filesystem/src/sync_ports.rs`) reads the asked block, then
starts `nearest_page_ancestor` at its parent with a memo that does not hold the asked row. When the
parent is the block itself, the walk reads the same row a second time.

## Missing piece

No deterministic case for a self-parented asked block; the property draws one only by chance.

## Remedy

`find_foreign_blocks` prefetches the asked row into the walk memo (`rows.prefetch`). New test
`a_self_parented_block_is_read_once` (`crates/holon-filesystem/tests/find_foreign_blocks_cost.rs`):
red before the fix with `left: 2, right: 1` (lane-logs/reds-fix/row6-red.log), green after
(lane-logs/reds-fix/row6-green.log, 13/13 with `nearest_page_ancestor_walk`). The property no longer
persists a seed file into the tree (`failure_persistence: None`), which had turned
`no_tracked_proptest_regressions` red as collateral.

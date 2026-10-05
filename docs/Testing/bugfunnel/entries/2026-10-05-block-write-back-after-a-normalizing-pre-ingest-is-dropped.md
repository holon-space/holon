---
id: 2026-10-05-block-write-back-after-a-normalizing-pre-ingest-is-dropped
date: 2026-10-05
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block-driven write-back whose pending-external-change pre-ingest wrote the
  file compared against the bytes from before that write, so it read as a user
  change and Holon's render was dropped without a retry.
---

## Bug
Found by code reading in lane d70d-write-back (ruling D70.d) and recorded as
ruling D86.a. The third verifier round (lane-logs/d70d-verify3.md, "D86.a
reachability", probe `p_h`) showed that the round-3 ingest stall arm made it
deterministic.

## Root cause
`on_block_changed_memoized` (crates/holon-filesystem/src/file_sync_controller.rs)
read the write-back basis before the pending-external-change pre-ingest
(`on_file_changed`). When that ingest wrote its normalization, the outer
compare-and-rename write compared against the older stamp and bytes, returned
`Changed`, and dropped the render (`BlockChangeVerdict::FileChanged`).
`schedule_reingest` keeps an existing `last_projection` (`or_insert`), which
the pre-ingest had just set to the bytes on disk, so no poll re-ingested the
file and nothing retried the render. The removal guards compared the render
against the same stale bytes. `re_render_all_tracked` had the same order.

## Missing piece
No test ran a block-driven write-back whose pre-ingest writes the file. The
keystone has no transition that leaves a file un-normalized on disk (an
external edit without its ids) right before a block edit to the same file.

## Remedy
FIXED. Both callers re-read the basis and the disk bytes after the pre-ingest,
so the write and the removal guard compare against the bytes they would
replace. Red first: `a_block_edit_lands_after_a_pre_ingest_that_normalizes_the_file`
in crates/holon-orgmode/tests/writeback_compare_and_rename.rs (the disk loses
its document id, so the pre-ingest writes; the holder carries an edit the
store render lacks). Red log lane-logs/d70d-r4-red.log: verdict `FileChanged`,
expected `Handled`. Green: lane-logs/d70d-r4-green.log.

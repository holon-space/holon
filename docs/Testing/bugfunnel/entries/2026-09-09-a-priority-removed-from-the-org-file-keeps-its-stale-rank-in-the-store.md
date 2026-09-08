---
id: 2026-09-09-a-priority-removed-from-the-org-file-keeps-its-stale-rank-in-the-store
date: 2026-09-09
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  An ingest that finds no priority carrier in an org file never cleared the
  stored rank, so a block whose priority was deleted from disk kept ranking by
  the value it no longer has — and the `RENDERER_VERSION` migration could not
  repair the blocks the drawer-erasure bug had already stripped.
---

## Bug

Found by the verifier on the `org-priority` lane (rev 1), reading the two
production write legs; reproduced end-to-end before the fix.

Delete a priority from an org file — or have the drawer-erasure bug
(`2026-09-08-a-drawer-authored-priority-is-erased-from-the-org-file-on-write-back`)
delete it for you — and the file no longer names a priority anywhere. Re-ingest
it and the store still ranks the block by the old value. Nothing downstream can
notice: the org file and the store simply disagree, forever.

It also holed the migration. `RENDERER_VERSION` was bumped so every org file
re-parses once through the corrected boundary, which repairs a block whose file
still carries a letter. A block whose file carries NO letter is exactly the one
the erasure bug produced — and re-ingest left its legacy (inverted, or
raw-string) rank untouched.

Reproduced (`lane-logs/rev2-RED-c.log`), ingest leg, through the real store:

```
FAIL holon-app::org_store_org_round_trip
     a_file_that_lost_its_priority_clears_the_stored_rank_on_re_ingest
  assertion `left == right` failed: a file with no priority carrier must clear
  the stored rank, not leave the legacy one
    left: Some(Priority(65))   # 'A', from the FIRST ingest
   right: None
```

## Root cause

Both write legs could only ever ADD a rank:

- `crates/holon-orgmode/src/block_params.rs` guarded the column behind
  `if let Some(priority) = block.priority()` with no `else`, so an absent
  priority emitted no param at all and the SQL merge kept the stored value.
- `crates/holon-loro/src/loro_sync_controller.rs` reaches `priority` only
  through the wholesale `block.properties` flatten in `block_to_params`, which
  has the same one-way shape; and `block_diff_params` emitted the
  `Value::REMOVED` property-removal sentinel, which edits the `properties` JSON
  bag and leaves the real `priority` COLUMN alone.

`collapsed`, `marks` and `source_language` had each already been given the
always-emit / explicit-NULL treatment for this exact reason. `priority` was
missed.

## Missing piece

**COVERAGE.** Every priority test in the tree authors a priority and checks it
arrives; none removes one. The round-trip PBT generates a document once and
renders it, so it never produces the two-ingest sequence (write a carrier, then
ingest the same block without one) that the defect needs. With no such
transition, no oracle could fire.

## Remedy

FIXED in the `org-priority` lane (rev 2). `Option<Priority>` now flows to the
column on both legs, `None` meaning clear:

- `build_block_params` always emits `priority`, `Value::Null` when the block
  carries none — the `collapsed` pattern;
- `block_to_params` defaults the column to `Value::Null` after the properties
  flatten;
- `block_diff_params` emits a column `Value::Null` rather than the property
  removal sentinel when `priority` disappears.

Covering tests (red above, green in `lane-logs/rev2-GREEN-final.log` and
`lane-logs/rev2-GREEN-final2.log`):

- `holon-app::org_store_org_round_trip::a_file_that_lost_its_priority_clears_the_stored_rank_on_re_ingest`
  — org → store (with priority) → re-ingest without one → the stored rank is
  gone, through the production provider;
- `holon-loro::loro_sync_controller::marks_outbound_tests::block_to_params_nulls_an_absent_priority`
  and `::block_diff_params_nulls_a_cleared_priority` — the Loro leg's create and
  update paths.

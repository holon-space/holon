---
id: 2026-10-14-a-restore-inside-the-write-stamp-crash-window-is-overwritten
date: 2026-10-14
gap: ORACLE
secondary: null
status: NOTED
summary: >-
  A user restore of an org file's exact pre-edit bytes, made after a crash
  between Holon's write and the stamp of its hash, is overwritten by the
  store's edit at the next boot.
---

## Bug

A block is edited in Holon and written to its org file. The process dies after
the write and before the file row records the written hash. While Holon is
down, the user restores the file to the exact bytes it held before the edit.
The next boot writes the store's edit over the restore.

Found by the lane verifier of the per-file write-back subnet
(holon-space/holon-work#175): the lane inverted the oracle of the base-green
test `a_restore_after_a_crash_before_the_written_hash_is_recorded_is_read_at_boot`
in the same commit that changed the behaviour.

## Root cause

The restored bytes equal the merge base, so the boot cannot tell them from a
file the crash left behind its store. Ruling WB-restore.a: disk == base and
store ahead means the store wins
(`crates/holon-filesystem/src/file_sync_controller.rs`, `store_may_be_ahead`).
The other choice reverts the store's edit in every "crash before the write
landed" case (hm4), which is the common case.

## Missing piece

The only test that pinned "a restore inside the crash window survives" was
edited in the same commit that changed the behaviour, so no gate could go red.
No gate flags a commit that inverts an existing test's assertion.

## Remedy

NOTED as a known limit (WB-restore.a, ADR 0032 §9). The test is now
`a_restore_inside_the_crash_window_between_write_and_stamp_keeps_the_stores_edit`
in `crates/holon-integration-tests/tests/org_suite/writeback_stale_cross_doc_prune.rs`;
its doc comment states the window. A disclosed conflict for this case
(option c) is a later card.

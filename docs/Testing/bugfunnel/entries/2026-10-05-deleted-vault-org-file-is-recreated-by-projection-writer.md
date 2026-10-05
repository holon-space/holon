---
id: 2026-10-05-deleted-vault-org-file-is-recreated-by-projection-writer
date: 2026-10-05
gap: COVERAGE
secondary: ENVIRONMENT
status: OPEN
summary: >-
  A vault org file the user deletes right after its first ingest is recreated
  by the projection writer, and its blocks stay in the database.
---

## Bug
Found while testing lane d45-followups (D66.a claim release). The test booted
a real session over a temp vault, wrote `profiles.org` (one block with a
profile source block), waited for the block to load, then deleted the file with
`std::fs::remove_file` and waited for the profile row to leave the database.
In 1 of 3 runs (and in every run when a second session booted at the same
time) the row never left: `profiles.org` existed again at the deadline and the
block was still in the database. With the file deleted again in a loop until
the block vanished, 5 of 5 runs passed.

A sibling observation: a user EDIT of the same file written shortly after the
first ingest was also lost in 1 of 8 parallel runs (the edit never reached the
profile load check: "the edit was never refused", log lane-logs/d66d-rep5.log).
It points at the same race, a write-back overwriting a fresh user write.

## Root cause
Measured with crates/holon-app/tests/vault_file_delete_after_ingest.rs, which
loops boot, write, wait for ingest, delete. 4 of 7 finished iterations failed.
Trace: lane-logs/frc-repro-1.clean.log (workspace b2-receiver-click). The cause
is in `ingest_file`'s normalization write-back
(crates/holon-filesystem/src/file_sync_controller.rs:6812-6865). The blocks
reach SQL before that step runs, so a delete that a test or user makes "after
ingest" can land inside the ingest. There are two outcomes:

- M1 (3 of 4 failures, plus the iteration that timed out): the TOCTOU re-read
  gets `NotFound`. The branch at :6852 returns `Ingested`. It does not record
  `last_projection` and does not cascade the delete. The page and its blocks stay
  in the store, and the path is not tracked. 17-45 ms later, the CDC feed's
  upsert of the page runs `page_identity_preflight` (:7074), then
  `materialize_page_identity_file` (:8485). That function sees no
  `last_projection` and an empty disk, so it writes the page file again
  ("Materialized identity file for runtime-created page ... -> profiles.org").
  The later watcher Remove finds the file present, so the delete is lost.
- M2 (1 of 4): the delete lands between the re-read (:6820) and `fs.write`
  (:6843). The normalization write recreates the file, and `last_projection`
  equals the new bytes, so the delete is lost. The same window lets this write
  overwrite a user edit that lands in it. That is the probable mechanism of the
  lost-edit sibling observation, but it is not measured.

Neither the watcher's echo suppression nor the base-diff path is involved. In
the passing runs, the delete arrived after the normalization write and was
cascaded correctly.

## Missing piece
`DeleteDocument` in the keystone deletes a file only after `CreateDocument` has
settled. `CreateDocument` writes a 0-byte file, which has no child block and no
normalization write-back. The SUT step (frontend_slice/components.rs:5239) waits
only for the page block to vanish. It never checks that the file stays deleted.
So no generated sequence deletes or edits a file while its first ingest is still
in flight. That is the coverage gap.

## Repro
1. Boot a session with `vault.root` set to an empty temp dir.
2. Write `profiles.org` with one headline holding a `holon_entity_profile_yaml`
   source block.
3. Wait until `SELECT id FROM block WHERE source_language =
   'holon_entity_profile_yaml'` returns a row.
4. Delete `profiles.org` once and poll the query for 30 s.
5. Expected: 0 rows and no file. Seen in 1 of 3 runs: the file exists again and
   the row remains. Logs: lane-logs/d66c-rep.log (the FAIL, "file exists
   again: true"), lane-logs/d66c-green.log (first failing runs).

## Remedy
Status stays OPEN: M1 is FIXED, M2 is open and needs a decision.

M1 FIXED. In `ingest_file`'s `NotFound` branch
(crates/holon-filesystem/src/file_sync_controller.rs, the TOCTOU re-read)
the ingest now records the ingested bytes as `last_projection`, as the sibling
TOCTOU branch does. The poll backstop and the watcher Remove then run
`on_file_deleted` and cascade. `materialize_page_identity_file` returns early
for a tracked path, so it no longer writes the file back.
- Red first: crates/holon-orgmode/tests/delete_during_first_ingest.rs
  (`a_file_deleted_during_its_first_ingest_is_cascaded_and_stays_deleted`).
  A FileSystem double deletes the file on the third read (the re-read).
  Red log: lane-logs/frc-m1-red.log (blocks stayed in the store). Green:
  lane-logs/frc-m1-green.log.
- Probe crates/holon-app/tests/vault_file_delete_after_ingest.rs, classified
  per iteration by lane-logs/frc-classify.py. Before the fix: 13 finished
  iterations, 10 failed (7 M1, 3 M2), 3 passed (lane-logs/frc-before*.clean.log).
  After the fix: 15 iterations, 1 failed (M2: "Wrote merged content" after the
  delete), 14 passed, 0 M1 (lane-logs/frc-after-?.clean.log).
- Keystone: the `DeleteDocument` SUT step now asserts the file stays deleted
  once the page block has vanished (frontend_slice/components.rs
  `delete_document`).

Remaining coverage gap: no transition deletes or edits a file while its first
ingest is still in flight. `DeleteDocument` runs only after `CreateDocument`
settled, on a 0-byte file with no normalization write-back. A new in-flight
delete transition is out of scope here.

M2 is open and needs a decision. Every write-back path does read, compare,
then write (also `on_block_changed` :7452 -> :7528), and no file-system step
makes that atomic. Options: (a) after the write, re-check and undo, which is
still racy; (b) no normalization write-back on first ingest, which defers it
to the next edit; (c) accept the window and disclose it.

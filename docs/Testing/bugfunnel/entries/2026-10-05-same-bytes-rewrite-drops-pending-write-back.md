---
id: 2026-10-05-same-bytes-rewrite-drops-pending-write-back
date: 2026-10-05
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  A file that gets a new stamp but keeps its bytes while Holon writes it back
  (touch, no-op save, sync-client rewrite) made the compare-and-rename
  write-back drop Holon's edit for good: the UI showed it, the org file did not.
---

## Bug
Found by the fresh-context verifier of lane d70d-write-back (ruling D70.d,
compare-and-rename write-back), by code reading
(lane-logs/d70d-verify.md, "D2").

`FileSystem::write_if_unchanged` compares stamps (inode, size, mtime), never
bytes. A process that rewrites a vault org file with the SAME bytes while
Holon renders a block edit (`touch`, `git checkout` of the same content, a
file-sync client's atomic rewrite, an editor's no-op save) changes the stamp.
The write-back returned `Changed` and dropped Holon's render R. The re-ingest
it scheduled never ran, because the poll re-ingests only on a CONTENT
difference and the disk still equalled `last_projection`. The recorded
`file.content_hash` still matched the disk, so a restart did not heal it
either. Result: the store and the UI hold R, the org file keeps the old bytes,
and nothing says so. The ingest normalization write-back had the same shape
(the file stayed un-normalized until the next event).

## Root cause
`write_back_or_skip_readonly` and the ingest normalization write
(crates/holon-filesystem/src/file_sync_controller.rs) treated any
`WriteBack::Changed` as a user change. A stamp change is not a content
change. The commit's own test file
(crates/holon-orgmode/tests/writeback_compare_and_rename.rs) only interfered
with byte-changing edits and deletes, so the case was never generated.

## Missing piece
No test interference rewrote a file with its own bytes during a write-back.
The keystone (general_e2e_composed_pbt.rs) cannot reproduce it either: it has
no transition for an external process that rewrites a vault file with
identical bytes, and its file-edit transitions never land inside a
write-back's read-to-rename window.

## Remedy
FIXED. `write_over_basis` (file_sync_controller.rs) wraps
`write_if_unchanged`: on `Changed` it re-reads the file, and when the bytes
still equal the basis bytes it retries against the new stamp (the render is
still valid). Only a real byte change or a delete returns `Changed` and goes
to re-ingest. The retry is bounded (`STAMP_CHANGE_RETRIES`); a file whose
stamp never settles is not written: the named error `StampChurn` raises a
write-back-stalled disclosure and a refused-write-back record for that file,
never a silent drop. Both the block-driven paths (through
`write_back_or_skip_readonly`) and the ingest normalization write use it.

The stall is per file (second verifier round, lane-logs/d70d-verify2.md,
"D3" and "D4"):
- On the ingest normalization write, `StampChurn` was first returned as an
  ingest error. That raised `VaultIngestFailed` instead of the write-back
  condition and quarantined a fully ingested file
  (`QuarantineCause::Ingest`), which stopped its block-driven write-back.
  Now the ingest discloses the stall and returns `Ingested`; the write is
  retried as a write (next bullet).
- D5, data loss (third verifier round, lane-logs/d70d-verify3.md, probes
  `p_i1`/`p_h`): the round-3 stall arm recorded the never-written render as
  the file's projection, so the retry was a RE-INGEST of the stale file
  against a base holding `:ID:`s the file lacks. It deleted the store block
  and recreated it under a new id, losing a store edit made during the stall
  ("Buy oat milk" gone, references dangling, nothing disclosed). Escape gap:
  COVERAGE, as above: the round-3 tests stopped the churn and polled, but
  never edited the store between the stall and the retry. Fix: the stall arm
  records the bytes it parsed (`disk_content`) as the projection, as the
  `WriteBack::Changed` arm does. (Round 4 also retried every stall from the
  poll; ruling D97.a removed that retry, see below.) The removal guard reads an ingest-stalled file's
  blocks from its stalled render (`removal_guard_source`), because the file
  bytes do not carry the ids the ingest minted, so an edited block would read
  as dropped. Red first: `a_store_edit_made_during_an_ingest_stall_survives_and_reaches_disk`,
  `a_block_edit_after_an_ingest_stall_reaches_disk` (lane-logs/d70d-r4-red.log:
  the poll during the stall deleted the block; the block edit never reached
  disk). Green: lane-logs/d70d-r4-green.log, with
  `an_edit_on_disk_during_an_ingest_stall_ingests_over_the_parsed_base`.
- `write_back_or_skip_readonly` returned `StampChurn` as an `Err`, and the
  bulk passes (`re_render_all_tracked`, `materialize_missing_page_files`)
  `?`-ed it, so one churning file ended the pass and the files after it
  (hash order) kept their old bytes. Now it returns
  `ProjectionWrite::Stalled` and the passes go on with the next file. The
  block-driven leg still returns the error for its one document.

### Ruling D97.a: a stall is skipped and disclosed, not retried

The fourth verifier round (lane-logs/d70d-verify4.md) found that the poll
retry of round 4 (`owed_writebacks`, `retry_owed_writebacks`) itself lost
data: D6 (a line the user deleted during an ingest stall was written back),
D7 (a line edited in the store and on disk during the stall became two
blocks on disk), D8 (the retry rendered from the store without the holder's
edit and retracted the disclosure) and D9 (a file deleted during the stall
left a permanent refused write-back, so every later shutdown failed).
Ruling D97.a: stop patching the stall path. A stalled write-back is skipped:
`disclose_stalled_writeback` logs a WARN naming the file, raises
`writeback_stalled` and records the document in `RefusedWritebacks`; the next
real event writes the file again. The retry machinery is deleted. The
per-file write-back state machine (plan `write-back-state-machine.md`) fixes
the stall path properly.

- Kept: the ingest stall arm tracks the file at the parsed bytes. Measured
  against leaving `last_projection` unset on a first-ingest stall
  (lane-logs/d70d-r5-variantA.log: 6 of 22 fail): unset, `poll_new_files`
  re-ingests the unchanged file every tick, writes it although no event
  came, turns a store edit into two blocks on disk, never cascades a delete
  of the file and retracts a pre-ingest stall. Parsed bytes:
  lane-logs/d70d-r5-variantB.log, 3 of 22 fail (the residuals below).
- Kept: `UnstampedFile` as the removal guard's source only. Without it
  (lane-logs/d70d-r5-no-unstamped.log) the ADR-0025 guard vetoes a block edit
  after an ingest stall as an ungrounded removal.
- D9 FIXED: `on_file_deleted_unsettled` retires the refused write-back and
  lifts the per-file condition after its cascade.
- D8's retraction FIXED: nothing but a landed write lifts the condition.
- Red first: lane-logs/d70d-r5-red.log (6 of 22 fail on the round-4
  controller: no WARN, the poll wrote, D6, D7, D8, D9). Green:
  lane-logs/d70d-r5-green.log (20 of 20; 3 ignored tests red for their
  reason).
- Residuals, OPEN, each an ignored test plus its own entry:
  2026-10-06-ingest-stall-undoes-a-line-deleted-on-disk.md (D6),
  2026-10-06-ingest-stall-splits-a-line-edited-in-two-places.md (D7),
  2026-10-06-pre-ingest-stall-loses-a-store-edit.md (D8).

Also: a block-driven write-back that found a real change now returns
`BlockChangeVerdict::FileChanged`, which di.rs does not count in
`docs_written_from_holder`.

Not changed, deliberate: when a page's home moves,
`materialize_page_identity_file` removes the old home file with an unstamped
`fs.remove` if it still carries the page's `#+ID:`, even when its bytes differ
from Holon's last projection (store-wins). A user edit to the old home made
in that window is lost. So "a write-back never destroys what the user wrote"
holds for overwrite and recreate, not for this delete
(docs/Architecture/Sync.md, FileFormatAdapter section).

- Red first: three tests in writeback_compare_and_rename.rs
  (`ingest_normalization_lands_over_a_rewrite_of_the_same_bytes`,
  `block_write_back_lands_over_a_rewrite_of_the_same_bytes`,
  `block_write_back_discloses_a_file_that_keeps_rewriting_its_own_bytes`).
  The double atomically replaces the file with its own bytes at write-back
  time. Red log lane-logs/d70d-fix-red.log: 3 of 3 fail ("the block write-back
  was dropped although the file kept the bytes it was rendered from", disk
  still "Buy milk"; churn returned Ok). Green: lane-logs/d70d-fix-green.log.
- Red first for the per-file stall: two tests in the same file
  (`ingest_normalization_discloses_a_file_that_keeps_rewriting_its_own_bytes`,
  `re_render_writes_the_other_files_while_one_file_keeps_rewriting_its_own_bytes`,
  12 rounds). Red log lane-logs/d70d-r3-red.log: 2 of 2 fail (the ingest
  returned the `StampChurn` error; the re-render pass returned it).

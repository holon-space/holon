---
id: 2026-10-10-overruled-file-text-lost-after-restart
date: 2026-10-10
gap: ORACLE
secondary: COVERAGE
status: FIXED
summary: >-
  A file edit that a Holon change overruled (TooLarge text merge, Deleted, Moved) was overwritten by the write-back and disclosed only in a process-lifetime condition, so after a restart the user's text was gone.
---

## Bug
Found by the verifier of the `merge-resurrect` lane (3-way text merge,
round 3). When Holon's change overrules a file edit — a text merge too large
to compute (`TextMergeOutcome::TooLarge`, Direct/SqlOnly mode), or an edit of
a block Holon deleted or moved (Loro mode) — the ingest keeps Holon's version
and the write-back replaces the file. The file's text survived only inside the
`FileEditOverruled` condition, which is sticky for the process. A restart
cleared the condition, and the text existed nowhere: not on disk, not in the
store.

## Root cause
The overruling sites in `crates/holon-filesystem/src/file_sync_controller.rs`
(`ingest_file`: the Deleted/Moved overrule loop and the TooLarge arm of the
updates pass) disclosed the file text through
`WritebackDisclosure::file_edit_overruled` and then let the write-back run.
Nothing persisted the overruled text.

## Missing piece
No test asserted that an overruled file text survives on disk across a
restart: `loro_delete_races_file_edit.rs` and
`file_sync_text_merge_keeps_deletes.rs` checked only the in-process
condition. The keystone generates no external edit racing a Holon delete or
move of the same block before its write-back, so its disk-content oracle
never saw the case.

## Remedy
Ruling D-conflict-copy.a: before an overruling write-back the controller
saves the whole file byte-equal as a conflict copy
`<stem>.conflict-<UTC YYYYMMDDTHHMMSSZ>-<NNN>.<ext>` beside it
(`FileSyncController::save_conflict_copy`, `holon_core::conflict_copy`), and
`FileEditOverruled` names the copy. `FormatRegistry::adapter_for` claims no
conflict copy, so no scan or watcher ingests it, and nothing deletes it. If
the copy cannot be written, the ingest fails and the existing quarantine
blocks the write-back and discloses the error. Pinned by
`edits_too_large_to_merge_keep_both_texts_and_disclose_the_files` and
`an_unwritable_conflict_copy_blocks_the_overruling_write_back_and_is_disclosed`
(`crates/holon-app/tests/file_sync_text_merge_keeps_deletes.rs`, with a
restart) and the `*_discloses_the_lost_text` cases of
`crates/holon-integration-tests/tests/loro_suite/loro_delete_races_file_edit.rs`
(with `stop_app`/`start_app`). Open: adoption conflicts (TakeDisk/TakeStore)
also overrule one side and get conflict copies next; the keystone coverage
gap (no generated external edit racing a Holon delete/move) stays open.

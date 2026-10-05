---
id: 2026-10-06-file-edit-racing-a-holon-delete-resurrects-the-block
date: 2026-10-06
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  An external save of an org file that lands after a Holon delete but before
  its write-back re-creates the deleted block, so the user's delete is lost
  with only an INFO line.
---

## Bug
Holon deletes a block. Before the write-back removes its line from the org
file, an external editor saves the same file (with an edit to another block).
The ingest of that save re-creates the deleted block in the Loro tree, the
store and the file. The delete is lost. The only trace is the INFO line
"re-seeded pre-Loro vault block into the Loro tree".

Found by the D69.a lane's race test
`crates/holon-integration-tests/tests/loro_suite/loro_delete_races_file_edit.rs`
(`a_delete_racing_an_external_edit_of_the_same_file_converges`), written for
ruling D91.a. A/B on main b2a19917: red on main too (pre-existing), with the
re-seed line in the log.

## Root cause
The ingest's upgrade-path re-seed adopted every pre-existing block for which
`Seen::is_absent_from_history()` was true. That predicate is true for
`Seen::Deleted` as well as `Seen::Never`, so a block deleted in the tree but
still on its stale org line was re-seeded as if a pre-Loro session had left
it (`crates/holon-filesystem/src/file_sync_controller.rs`, creates pass).
`Seen::admits_adoption()` (only `Never`) is the predicate the type documents
for adoption.

## Missing piece
The keystone settles after each transition, so a Holon delete is always
written back before the next external edit. No transition interleaves an
external save between a Holon change and its write-back.

## Remedy
The ingest classifies each block of the last-ingested base against the tree
once: only `Never` re-seeds. A block Holon deleted or moved since is skipped
with a WARN naming block and file; the write-back brings the file in line. If
the file also changed that block's text, the lost text is disclosed as the
`FileEditOverruled` condition. Pinned by the four cases of
`loro_delete_races_file_edit.rs`. A keystone transition that saves a file
between a Holon change and its write-back stays open.

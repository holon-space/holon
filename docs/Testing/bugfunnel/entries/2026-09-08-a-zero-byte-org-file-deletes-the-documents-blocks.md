---
id: 2026-09-08-a-zero-byte-org-file-deletes-the-documents-blocks
date: 2026-09-08
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The 0-byte intermediate of an editor's atomic save was ingested as an
  empty document, deleting every block of the document that lives at that
  path and writing the empty projection back over the file.
---

## Bug

Martin's production vault carried 0-byte `Journals/2026-04-20.org` and
`Journals/2026-04-30.org`, and the running app logged at ~5 Hz against them.
Recorded in the vault as the symptom record under `Plain-Text Layer.org`
(`:ID: fb1a49a2-fbc1-487c-9cbb-1d9cc1073d11`), backlog row `empty-doc-skip-watcher`.
Found by Martin dogfooding; this lane (`plaintext-layer`) fixed it.

A 0-byte file is not only a file a user left empty. Every editor that saves
atomically — write a temp file, rename it over the target — and every writer
that truncates before writing makes the target observable at zero length
first. The watcher sees that moment as an ordinary content change.

Reproduced headlessly against the real controller with the real org adapter,
in `crates/holon-orgmode/tests/empty_file_is_not_a_document.rs`. Two distinct
harms, both red before the fix
(`.claude/worktrees/plaintext-layer/lane-logs/red3-88231.log`):

```
a_zero_byte_save_intermediate_leaves_the_documents_blocks_alone
  a 0-byte file was ingested as an empty document and deleted the blocks that
  live at its path — the save's real bytes had not even landed yet
    left: []
   right: ["block:20260908T090001", "block:20260908T090002"]

a_zero_byte_file_is_not_written_back
  Holon rewrote a file whose save was still in flight
    left: "#+ID: 20260908T090000\n"
   right: ""
```

## Root cause

`FileSyncController::ingest_file`
(`crates/holon-filesystem/src/file_sync_controller.rs:2727`) reads the file
and hands whatever it read to the format adapter. Empty content parses to a
document with zero blocks, and the ingest reconciler turns "zero blocks
parsed, N blocks stored" into N `delete` ops applied through
`apply_ingest_batch` (`:3929`). Nothing between the read and the parse asks
whether the bytes can identify a document at all — and they cannot: an empty
file carries no `#+ID:`, no `#+TITLE:`, and no drawer.

The write-back leg then compounded it: with the document emptied, the
projection of that path is an `#+ID:` line, which was rendered over a file
whose real bytes were still being written.

The ERROR storm Martin saw is the same event repeating: the file stays
0 bytes for as long as the writer holds it, and neither the notify path nor
the 2 s discovery tick had any record that would stop it re-deciding.

## Missing piece

No generator ever authors a 0-byte file. The keystone PBT's vault fixtures
are always parseable documents, and its file writes are single `write_all`
calls with content — so the atomic-save shape (a path that is momentarily
empty and then not) is ungeneratable, not merely unlikely. That is the
COVERAGE gap.

Secondary, ENVIRONMENT: the keystone drives its filesystem through
`InMemoryFileSystem`, which no editor is writing to concurrently. The
truncate-then-write window is a property of real editors on a real vault,
and there is no rung that models a writer holding a path mid-save.

Prod/test parity work that would close it properly: a vault-file transition
that writes a file in two observable steps (empty, then content) with a
watcher pass between them, so every ingest-side invariant is asked about the
intermediate state as well as the final one.

## Remedy

Fixed in `crates/holon-filesystem/src/file_sync_controller.rs`.

`ingest_file` refuses a 0-byte file ahead of echo suppression and the parse,
returning the new `IngestOutcome::RefusedEmptyFile`. The refusal is a skip,
not an `Err`: the ingest loop keeps serving every other file, matching the
containment posture of the duplicate-`#+ID:` refusal beside it. It is
disclosed once per file per signature at INFO, naming the path and saying
what ends the skip.

`poll_new_files` records the refusal in `ingest_quarantine` at the file's
`(mtime, size)` signature, the same machinery a duplicate-id refusal uses.
Without it an untracked empty file is rediscovered, re-read and re-decided
every 2 s discovery tick — the storm again, one layer down. The bytes of the
save in flight change the signature, which lifts the quarantine on the next
tick; nothing has to time out or expire.

For a file already tracked, `poll_tracked_files` stamps `disk_signatures`
before the diff, so the skip is likewise decided once per signature rather
than per tick.

Deliberately 0 bytes and not `trim().is_empty()`: a file holding only
whitespace is a file someone wrote, and treating it as absent would silently
refuse a real (if degenerate) edit. The atomic-save intermediate is
specifically zero-length.

Pinned by `crates/holon-orgmode/tests/empty_file_is_not_a_document.rs`, whose
ordering double APPLIES `delete_in_tree` — a double that swallows deletes
cannot see this bug at all, which is why the neighbouring harnesses (whose
`delete_in_tree` is `Ok(())`) stayed green through it.

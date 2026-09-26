---
id: 2026-09-26-a-headline-moved-between-org-files-is-lost-when-the-target-is-ingested-first
date: 2026-09-26
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A headline cut from one org page file and pasted into another disappears
  from the store and from both files when the target file is ingested before
  the source file.
---

## Bug
Found by an agent probe in lane `ingest-clear-props` (round 2). The probe
looked for the ingest's re-parent path through the real `FileSyncController`
(`TestEnvironment`, both storage arms). Output: `lane-logs/reparent-probe.log`.

Setup: `Alpha.org` holds `* TODO [#A] Buy milk` (`:ID: moved-h`). The same
headline is moved to `Beta.org`, and both files are written.

| Write order | Result, SqlOnly and Loro |
|-------------|--------------------------|
| `Alpha.org` first, then `Beta.org` | the block is deleted, then created under `page-beta`; `Beta.org` keeps it |
| `Beta.org` first, then `Alpha.org` | after `Beta.org`: the block is still under `page-alpha`, and it is pruned from `Beta.org` on disk; after `Alpha.org`: the row is gone (`final=None`), and `Beta.org` holds only its header |

In the second order the moved headline exists nowhere any more. An editor or
a sync tool can write the two files in either order.

## Root cause
The cross-doc membership guard in `ingest_file`
(`crates/holon-filesystem/src/file_sync_controller.rs`, the
`stale_cross_doc_ids` block) treats a parsed block that is authoritatively
owned by another page as a stale on-disk copy. It skips the block and prunes
it from the ingesting file's write-back. That is correct for the
journals-phantom shape (`writeback_stale_cross_doc_prune.rs`). It is wrong
for a move: when the source's own ingest then drops the block, nothing holds
it. At the target's ingest, a stale copy and a move in progress look the
same.

The mechanism is outside the lane's diff; this is read from the code and
from the probe, not A/B-run on the base revision.

## Missing piece
The keystone could not move a block between files. `WriteOrgFile`'s
precondition rejects a file that carries an id another document owns
(`BlockIdAlreadyExists`), and no other transition wrote two files that
exchange a block.

The keystone now moves a block between files. `MoveBlockBetweenFiles`
saves both files back to back (source first, or target first).
`PasteBlockCopy` pastes a copy at the top of the target file and saves only
the target, so the invariants run in the two-copies state: `inv-copies-stay-on-disk`
checks both copies on disk, and `inv-conditions-match-ref` checks the
condition, its owner file and its copy file. While a copy stands, Holon-side
edits (task state, typing, splits, joins, indents), block creation, further
pastes, `EditBlockCopy` (the editor changes the copy's task keyword) and
`DeleteDocument` stay enabled. `FinishCutPaste` then saves the source (the
copy is adopted, merged with Holon's edits) or deletes the copy (the block
stays). The model's copy map (`crates/holon-integration-tests/src/pbt/copies_model.rs`)
filters the org observations and states the conditions after every
transition, so the condition kinds are governed in every run.
`WriteOrgFile`'s precondition is unchanged: it seeds files before start, where
two files with one id is the phantom shape, not a move.

Red on the base revision `c7dda4ad`, for the right reason, in both storage
arms: `inv-blocks-match-ref/block_raw` reports INGEST DATA LOSS of the moved
block (`lane-logs/d229-red-sqlonly.log`,
`lane-logs/d229-red-target-ingested-before-source-loro.log`,
`lane-logs/d229-red-target-saved-first-sqlonly.log`). The source-first order
was green there, as the probe predicted.

## Remedy
FIXED by ruling D229.b, in `crates/holon-filesystem/src/file_sync_controller.rs`
(design: `lane-logs/d229-design.md` of lane d229-move).

- **Release is an event.** Only the owner file's own ingest proves that it
  let a block go: the block was in the bytes Holon last knew of that file and
  is not in its new parse, or the file was deleted. Another file's ingest
  never reads the owner's file to decide. A block Holon has not written to a
  file yet can therefore never be released by it.
- **Adoption merges.** The file with the copy adopts the released block,
  merged field by field with the store against the pasted version (text by
  3-way merge on both storage arms). When both sides changed a field apart,
  the adoption is refused: the block goes back to its owner's file, both
  copies stay, and `BlockEditedInTwoFiles` is shown. The user's next deletion
  decides (deleting it from the owner's file again adopts the copy).
- **No write-back is held.** A write of a file with a copy renders the store
  and keeps each copy subtree's disk bytes at its disk position (org
  `keep_subtrees`), so Holon's edits reach that file at once.
- **Holon delete of a copied block** brings the block back from its copy as
  that file's own block, with `DeletedBlockKeptInFile`.
- **Boot:** a file the initial scan has not read yet that holds a released
  block is read at once, so a block moved while Holon was closed is adopted,
  never deleted and re-created, in either scan order.

No copy is removed from disk by Holon. The journals-phantom pin
(`org_suite writeback_stale_cross_doc_prune`) asserts "disclosed, kept, no
oscillation" and pins the empty-owner, missing-owner, three-file,
copied-subtree, Holon-edit-survives-adoption, edits-reach-a-copy-file,
conflict, Holon-delete and moved-while-closed shapes.

Replayed by sixteen `hand-authored-regressions/keystone.jsonl` cases
(`headline-moved-between-files-*`, `pasted-copy-deleted-by-hand-*`,
`copied-block-toggled-in-holon-keeps-the-toggle-when-adopted-*`,
`block-created-under-a-copy-file-is-copied-not-released-*`,
`copy-conflict-ends-with-the-next-deletion-*`,
`owner-file-deleted-while-a-copy-stands-*`,
`copied-block-deleted-in-holon-comes-back-from-its-copy-*`).

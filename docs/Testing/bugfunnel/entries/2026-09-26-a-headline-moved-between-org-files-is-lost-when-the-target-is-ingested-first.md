---
id: 2026-09-26-a-headline-moved-between-org-files-is-lost-when-the-target-is-ingested-first
date: 2026-09-26
gap: COVERAGE
secondary: null
status: OPEN
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
The keystone cannot move a block between files. `WriteOrgFile`'s
precondition rejects a file that carries an id another document owns
(`BlockIdAlreadyExists`), and no other transition writes two files that
exchange a block.

## Remedy
OPEN. This needs a decision: how the ingest tells a move from a stale copy.
Two examples: defer the prune until the owning file was re-read, or adopt
when the owning file no longer carries the block on disk.

---
id: 2026-08-31-marks-written-against-stale-content-quarantines-file
date: 2026-08-31
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  The ingest `marks` field write re-asserts the block's OLD stored content, so
  mark spans from the new parse fall outside the text and Loro rejects them,
  failing the ingest and quarantining the file from write-back.
---

## Bug

Found in Martin's 2026-08-31 session log (`/tmp/holon-cold.log`) while
diagnosing the `ClaudeCode` page. Four separate occurrences over 18 hours, on
two files under `holon-pkm/Agents/cc/` (the claude-history session mirrors —
long, frequently appended-to org files):

```
08:13:52 ERROR [FileSyncController] ingest FAILED partway — QUARANTINING this
  file from write-back ... error=apply_ingest_batch ...
  BlockCellRegistry::write_field(marks): update_block_marked(block:...):
  Failed to update block marked: LoroText mark bold:
  OutOfBound { pos: 178, len: 72 }
```

Observed span/length pairs: `pos 178 len 72`, `pos 204 len 197`,
`pos 251 len 212`, `pos 213 len 151`. Each failure re-logged 3-4 times as it
propagated through the file feed, the block feed and `poll_tracked_files`
(`holon_orgmode::di`) — 15 of the session's 20 ERROR lines come from these
four events.

Consequence is real, not cosmetic: the file is quarantined from write-back
until a later ingest fully succeeds, so the DB holds a truncated version of
the document and Holon deliberately refuses to render it over disk.

## Root cause

`crates/holon-loro/src/block_cell_registry.rs:821-844`. The `marks` field is
written as its own cell, and to satisfy `update_block_marked`'s
"text + marks wholesale replace" contract it fetches the text itself:

```rust
let current = backend.get_block(&id).await?;      // :836-839  OLD content
backend.update_block_marked(&id, &current.content, &marks).await   // :842
```

`update_block_marked` (`crates/holon-loro/src/loro_backend.rs:2378` onward)
then sets `CONTENT_RAW` to that old text and applies the new spans over it
(`:2394-2399`). When the same ingest batch also carries a longer `content`
for the block and the `content` cell has not been written yet, the mark spans
are indexed against the NEW string while the text is the OLD one, and Loro's
`text.mark(start..end, ...)` rejects the range.

So the defect is a within-batch field-ordering assumption: `marks` silently
depends on `content` having landed first, and nothing enforces or documents
that. The two cells are independent writes.

The mismatch is also detected too late and too far away — Loro reports it as
an internal `OutOfBound` at `handler.rs:2436` rather than Holon asserting
`span.end <= len_chars` at the boundary with the block id, both lengths and
the offending mark.

## Missing piece

The keystone generates content edits and mark edits, but it has no transition
that produces **a single ingest batch carrying both a longer content and
marks positioned in the added tail** for one block. Every mark transition in
the catalog operates on a block whose content is already at its final length,
so the ordering hazard is never expressed — a COVERAGE gap.

Secondary ORACLE gap: no invariant asserts "an ingest that fails partway
leaves no file quarantined", so even a case that hit this would only have
shown up as a log line, not a red.

## Remedy

Open. Fix direction:

1. Make the pairing explicit rather than ordering-dependent: `marks` should be
   written from the batch's content, not from `get_block`. Either write
   content+marks as one cell (a `content_marked` field), or have
   `apply_ingest_batch` order `content` before `marks` and assert it.
2. Add a loud precondition in `update_block_marked` before the `mark` loop —
   `span.end <= len_chars`, erroring with block id, text length and the span —
   so the failure names the mismatch instead of surfacing a Loro internal.
3. Close the coverage gap first: add a keystone transition that appends to a
   block's content and marks a range inside the appended tail in the same
   batch. It should go red with this exact `OutOfBound` before the fix.

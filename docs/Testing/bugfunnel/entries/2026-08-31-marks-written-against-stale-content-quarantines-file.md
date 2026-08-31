---
id: 2026-08-31-marks-written-against-stale-content-quarantines-file
date: 2026-08-31
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The ingest `marks` field write re-asserts the block's OLD stored content, so
  mark spans from the new parse miss the text: past its end when the content
  grew (Loro rejects, the ingest fails and the file is quarantined), or silently
  shifted to wrong offsets when it shrank.
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

FIXED.

**Root fix** — `crates/holon/src/core/sql_block_operations.rs`: the Upstream
branch of `update_in_tree` decomposes the ingest bag into per-field
`set_field` calls, and the bag is a `HashMap`, so `content` and `marks` landed
in per-call RANDOM order. The new `ingest_field_write_order` (same file, just
above the `BlockOrdering` impl) states the order the pairing needs — `content`
first, `marks` last, everything else alphabetically in between — so the marks
write can no longer address the previous revision's text. `content` and `marks`
always travel in the SAME bag (`build_block_params` emits `content`
unconditionally), so ordering is a complete fix, not a narrowing.

**Loud precondition** — `crates/holon-loro/src/loro_backend.rs`,
`update_block_marked`: every span is checked against the text THAT CALL
installs (`span.end <= new_text.chars().count()`) before Loro sees it. A future
desync now reads

    update_block_marked(block:…): mark span 9..17 (bold) is out of range for
    the 12-char text this call installs — the mark set was derived from content
    other than `new_text`

instead of a Loro-internal `OutOfBound { pos, len }` that names neither the
block nor the string it was measured against.

Quarantine itself is untouched — it is the correct last resort, and the bug was
upstream of it.

## The silent variant

The quarantine was only the LOUD half. The same random field order, applied to a
batch that SHORTENS the content, writes the new spans over the old, LONGER text —
where they are in bounds, so Loro accepts them, nothing errors, and no file is
quarantined. The content write that follows then shifts those Peritext anchors,
leaving the block with marks at offsets it was never given (verifier probe:
stored `0..7` against a real `4..12`, on 2 of 3 pre-fix runs). The write-order
rule fixes both directions; the rung `marks-shortening-batch` covers this one.

This bounds the recovery answer above. **Restarting clears the quarantine, but
nothing repairs mark spans that a pre-fix session already shifted.** A shifted
span is in bounds and structurally indistinguishable from an authored one, so
there is no retroactive detection and no migration to write — the corruption is
silent by construction. Only a re-ingest of the file re-derives the marks from
the org source, which the fix now makes correct.

## Rungs

- `marks-lengthening-batch`
  (`crates/holon-integration-tests/tests/boot_suite/marks_lengthening_batch_no_quarantine.rs`)
  — the missing rung this entry named. Drives the REAL
  `BlockOrdering::apply_ingest_batch` (Loro/Upstream, params from
  `holon_orgmode::build_block_params`) with forty successive revisions that each
  lengthen the content and mark the appended tail. Driving the batch seam
  directly rather than the file watcher is deliberate: the controller's poll
  retry re-rolls the `HashMap` order and hid the defect end-to-end.
- `marks-shortening-batch` (same file) — the SILENT direction: twenty SHORTENING
  revisions, checked after EVERY one, because a later correctly-ordered revision
  rewrites the mark set and would paper over an earlier corruption. Green
  deterministically with the rule; red with probability 1 - 2^-20 without it,
  measured 3 of 3 runs (rounds 17 / 18 / 19) with the rule bypassed.
- `marks-span-precondition`
  (`crates/holon-app/tests/marks_span_precondition.rs`) — the precondition
  rejects an out-of-range span by name, and still applies an in-range one over
  the newly installed text.
- `content_is_written_before_marks`
  (`sql_block_operations.rs`, `ingest_field_order_tests`) — a hundred fresh bags,
  deterministic.

Red-for-the-right-reason (fix reverted, rungs in place), round 1 of 40:

    round 1: the ingest batch failed partway, so `FileSyncController` would
    QUARANTINE this file from write-back … error:
    BlockCellRegistry::write_field(marks): update_block_marked(block:…): mark
    span 9..17 (bold) is out of range for the 12-char text this call installs

**Fixture note**: the marked word must CLOSE the block body. A first fixture put
`" here."` after it — six trailing characters against five characters of growth
per revision, so the stale text stayed long enough to swallow the span and the
rung passed green with the fix reverted.

## Recovery of an already-quarantined file

Nothing manual is required, and nothing was lost: quarantine skips WRITE-BACK,
so the intact file on disk was never overwritten — that is the point of the
guard.

The quarantine set is `FileSyncController.quarantined`
(`crates/holon-filesystem/src/file_sync_controller.rs:713`), a plain in-memory
`HashMap` built empty at construction (`:851`) and never serialized. So it does
not survive a restart, and within a session the next fully-successful ingest of
that path clears it and logs `write-back quarantine CLEARED` (`:2073-2080`).

With this fix the re-ingest succeeds, so **restarting Holon is the whole
recovery**: boot drops the in-memory flag and re-ingests the file cleanly. A
running instance heals on its own the next time the file is touched.

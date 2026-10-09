---
id: 2026-10-09-file-sync-3way-merge-resurrects-deleted-text
date: 2026-10-09
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  The Direct-mode file-sync 3-way text merge brought back text the user deleted
  in the app when the same block's org file was edited before the write-back.
---

## Bug
Found by the M5 stale-keystroke rebase spike (lane loro-seed-spike, S3
verdict, `lane-logs/s3/VERDICT.md` and `debug-3way.log` in that workspace),
which measured `TransientLoroTextMerge::merge_text` as one rebase candidate.
Base `abcdef`, file `abcABCeDf`, app `abf` merged to `abABCeDf`: the `e` the
app deleted came back. Base `""`, file `SS`, app `k` gave `SSk`, with no
written rule for the order of concurrent inserts into one gap.

Users reach it through `FileSyncController` in Direct (SqlOnly) mode: the
ingest of a changed org file whose block the store also changed
(`three_way_text_content`, crates/holon-filesystem/src/file_sync_controller.rs)
and the adoption merge of a block (`merge_adopted`, same file). The merger is
wired in crates/holon-app/src/wiring.rs and crates/holon-orgmode/src/di.rs.

## Root cause
`merge_text` applied each side to a Loro text with `LoroText::update`, which
does not give a minimal diff: it encoded `abcdef` → `abcABCeDf` as "delete
`de`, insert `ABCeD`". The file's `e` was then a new char that the app's
delete of the base `e` could not reach. The peer ids put the file side (1)
before the app side (2) at an equal position.

## Missing piece
No test reached the merge with a real merger: the controller tests used a stub
merger, and `merge_text` had two example tests. The keystone settles each step
(write-back included) before the next, so an external edit never meets an
in-app edit that is not yet on disk, and its reference model has no 3-way
text merge to compare against.

## Remedy
Fixed: each side is applied as the minimal (Myers) diff of grapheme clusters
from base (`similar`), each hunk inserting before it deletes, and the app side
gets the lower peer id, so same-gap inserts come out app first, then file. The
edit positions come from a running cursor over the raw Myers ops:
`similar::capture_diff_slices` compacts its ops, and with repeated chars their
positions disagree with their order (`a ` → ` b ` put `b ` in the wrong gap,
so `merge("a ", "a ", " b ")` gave `b  `). Myers costs O((n + m) * d), so a
side's diff is bounded by a work limit on `(n + m + 1) * (d + 1)`, checked by a
capped greedy edit-distance search before the diff runs. A side over the limit
makes the outcome `TextMergeOutcome::TooLarge`, and nothing is merged: the
ingest keeps Holon's text and discloses the file's text as a
`FileEditOverruled` condition (`HolonChange::Edited`); the adoption merge
counts the text as a conflicting field. The outcome depends on the three texts
alone, never on wall time or on which side is which. A diff of lines first is
not used: it is not minimal (`"\n "` to `"a\n \n\n"` replaced the kept `" "`).
The contract is on `TransientLoroTextMerge`
(crates/holon-loro/src/text_merge_provider.rs).
Pinned by the property test crates/holon-loro/tests/transient_text_merge_pbt.rs
(unique-char triples: no resurrection, no lost edit, per-side order, exact
same-gap order; repeated-char texts: an unchanged side yields the other
exactly, char counts against an insert-only side, insert-only sides both
survive; under any work limit a merge keeps those properties or is
`TooLarge` for both side orders; 60 000-char sides with many edits and a full
rewrite) and the controller tests
crates/holon-app/tests/file_sync_text_merge_keeps_deletes.rs (production
controller and merger; a rewrite too large to merge keeps Holon's text and
puts the file's text on the condition bus). Strings do not say which of equal chars a side
deleted, so both sides deleting one `b` of `bbb` counts as one delete.
Open: a keystone transition that holds the write-back while the same block is
edited in the app and in its file, with a reference 3-way merge.

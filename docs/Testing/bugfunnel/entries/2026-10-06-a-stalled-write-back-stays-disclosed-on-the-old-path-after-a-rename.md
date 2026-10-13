---
id: 2026-10-06-a-stalled-write-back-stays-disclosed-on-the-old-path-after-a-rename
date: 2026-10-06
gap: COVERAGE
status: FIXED
summary: >-
  A store edit whose write-back stalled stays disclosed on the file's old path
  after the user renames the file, and the owed edit does not reach the
  renamed file when the churn ends.
---

## Bug
Found while building the red keystone rows of the per-file write-back state
machine (`write-back-state-machine.md`, ruling D97.a, invariant I7), lane
wb-states. The plan predicted it from the code; the sidecar case
`wb-i7-stalled-files-disclosure-follows-a-rename` in
crates/holon-integration-tests/hand-authored-regressions/known-red-writeback-state-machine.jsonl
is the first run that reaches it.

Scenario: `wb.org` churns (its stamp keeps changing over the same bytes); the
user types ` oat` into `Buy milk`; the block write-back stalls and is
disclosed as `writeback-degraded` on `wb.org`. The user renames the file to
`wb-renamed.org`. The churn ends. At `DisarmWriteChurn`
(`inv-conditions-match-ref`) `writeback-degraded` is in effect on BOTH
`wb.org` (a path that no longer exists) and `wb-renamed.org`, and
`inv-org-render-fixed-point` shows `Buy milk` on disk while the store renders
`Buy milk oat`.

## Root cause
Not measured. Observations: the stall disclosure is keyed by path
(`disclose_stalled_writeback`, crates/holon-filesystem/src/file_sync_controller.rs),
and only a write that lands at that path retires it (`writeback_resumed`). A
rename moves the document to a new path, so nothing retires the old path's
condition. When the churn ends, no event re-renders the document, so the owed
edit stays off disk (the same shape as the D8 row
`wb-d8-stalled-edit-never-reaches-disk`).

## Missing piece
COVERAGE: no keystone transition made a file's stamp churn, so no stall was
ever combined with a rename. The new transitions `ArmWriteChurn` and
`DisarmWriteChurn` reach it.

## Remedy
FIXED by the per-file write-back subnet (ADR 0032 §9,
holon-space/holon-work#175). A rename re-points the owed record's home and
re-reads a refused file, so the disclosure follows the file and the owed edit
lands in the renamed file. Pinned by keystone case
`wb-i7-stalled-files-disclosure-follows-a-rename` in
`crates/holon-integration-tests/hand-authored-regressions/keystone.jsonl`.

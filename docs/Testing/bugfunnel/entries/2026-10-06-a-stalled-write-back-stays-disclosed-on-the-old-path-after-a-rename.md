---
id: 2026-10-06-a-stalled-write-back-stays-disclosed-on-the-old-path-after-a-rename
date: 2026-10-06
gap: COVERAGE
status: OPEN
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
OPEN. Registered known red `wb-i7-stall-disclosure-keeps-old-path`
(docs/Testing/KeystoneKnownReds.md). Red log lane-logs/a4-red-wb-i7.log
(wb-states lane). In the write-back state machine plan the disclosure is
derived from the per-file record (invariant I7), and that plan owns the fix.

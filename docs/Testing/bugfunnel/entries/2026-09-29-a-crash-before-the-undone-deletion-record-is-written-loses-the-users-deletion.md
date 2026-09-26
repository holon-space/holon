---
id: 2026-09-29-a-crash-before-the-undone-deletion-record-is-written-loses-the-users-deletion
date: 2026-09-29
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A crash between Holon's put-back of a deleted child and the write of the undone-deletion record leaves the child back with no record, so the user's deletion is lost silently.
---

## Bug
Found by reading by the D229 round-6 verifier (lane d229-move), not driven. Report: `lane-logs/d229r6v-verify.md`, Defect B.

## Root cause
`on_file_changed` wrote the put-back to the store and the file before `persist_undone_deletions` wrote the record.

## Missing piece
The keystone has no crash between two writes of one ingest; the test environment had no crash seam.

## Remedy
Fixed in round 7: the record is written ahead of the put-back (R11.3), with the `crash-injection` seam and `an_undone_deletion_survives_a_crash_*` tests. The adoption arm stayed write-behind until round 8 (entry 2026-09-30-an-adopted-copy-ends-the-undone-deletion-record-before-its-change-is-durable).

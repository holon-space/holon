---
id: 2026-07-17-external-whole-file-deletion-still-per
date: 2026-07-17
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  External whole-file DELETION still per-op: `on_file_deleted`
  (file_sync_controller.rs:682/697) deletes blocks one at a time, retaining
  the per-block matview-maintenance O(N²) cost class that the row-32 batching
  fix removed for ingest — deleting a large file from disk will stall
  proportionally to vault size
source_line: 815
---

## Bug

External whole-file DELETION still per-op: `on_file_deleted`
(file_sync_controller.rs:682/697) deletes blocks one at a time, retaining
the per-block matview-maintenance O(N²) cost class that the row-32 batching
fix removed for ingest — deleting a large file from disk will stall
proportionally to vault size

## Missing piece

no batched delete path; no test asserts maintenance-pass count for file
deletion (sibling of the row-32 count tests)

## Remedy

OPEN — flagged by the ingest-batching verifier 2026-07-17; fix = route
on_file_deleted through apply_ingest_batch-style single transaction

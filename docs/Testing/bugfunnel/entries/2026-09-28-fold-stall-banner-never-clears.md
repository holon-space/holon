---
id: 2026-09-28-fold-stall-banner-never-clears
date: 2026-09-28
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  A file's write-back stall banner (`WritebackDegraded`) stayed raised after
  the fold converged and writes reached disk again.
---

## Bug
Found by the org-faithful group B r2 verifier (V4, `lane-logs/groupB-r2-verify.md`).

## Root cause
No emitter cleared `WritebackDegraded` for a file subject.

## Missing piece
No test covered the stall's all-clear.

## Remedy
Every successful write-back of a file calls
`WritebackDisclosure::writeback_resumed`, which clears the file's
`WritebackDegraded`, and resets the stall latch. Pinned by
`sync_controller_mutation_pbt.rs` (`a_stalled_file_is_cleared_by_its_next_write`).

---
id: 2026-10-27-the-initial-vault-scan-is-quadratic-in-files
date: 2026-10-27
gap: ENVIRONMENT
secondary: ORACLE
status: FIXED
summary: >-
  The initial vault scan took time quadratic in the file count, because every write-back net step decided every auto transition again for every document.
---

## Bug
A measurement of the asset seed's header pass found it: its bar is a share of the controller's initial scan, and the scan of 1000 files took a median 235,694 ms (test profile). The first projection waits for the scan, so a large vault boots far above the latency SLO.

## Root cause
`FileSyncController::on_file_changed` (crates/holon-filesystem/src/file_sync_controller.rs) drives the write-back net after each ingest (`drive_writeback_net`, file_sync_controller/writeback.rs). Each `Executor::step` ran `quiesce` (crates/holon-net/src/exec.rs), which called `decide` (bind, trace, mark hash) for every subject of every auto transition. Each ingested document adds a subject, so file k cost O(k). Measured before the fix (release, `initial_scan_cost`): medians 47,312 / 101,307 / 429,809 ms for 1k / 2k / 4k files, a 4k/1k ratio of 9.08.

## Missing piece
Litmus ENVIRONMENT: the code path runs in the keystone, but no test drives it at vault scale; the keystone's few documents hide a per-step cost that grows with the subject count. Litmus ORACLE (secondary): no test bounded the scan's growth in the file count.

## Remedy
The executor keeps a set of (transition, subject) pairs to decide again: those whose own rows changed, those whose last refusal read a changed row, those whose failure mark changed, and every subject of a transition whose pool changed. A quiescence decides only these. Pinned by crates/holon-net/tests/exec_redecide.rs and the release measurement `the_initial_scan_grows_linearly_with_the_file_count` in crates/holon-orgmode/tests/initial_scan_cost.rs.

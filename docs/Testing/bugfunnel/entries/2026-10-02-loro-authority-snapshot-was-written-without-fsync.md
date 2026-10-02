---
id: 2026-10-02-loro-authority-snapshot-was-written-without-fsync
date: 2026-10-02
gap: COVERAGE
secondary: ENVIRONMENT
status: FIXED
summary: >-
  The Loro snapshot, which is the authority for the vault, was written with
  write-then-rename and no fsync, so a power loss could leave an empty or
  stale snapshot although the save had returned and the SQL projection already
  showed the change.
---

## Bug
Found by a code audit while building the Loro update log (deep-vault lane,
increment 6), not by a test. The save path doc claimed "snapshot + fsync".
`LoroDocumentStore::save_all` wrote the snapshot with `write_atomic_blocking`.

## Root cause
`write_atomic_blocking` (`crates/holon-filesystem/src/fs_port.rs`) renames a
temp file over the target and never calls `sync_all` on the file or on its
directory. That is a deliberate choice for the org mirror, which can be
derived again from the authority. The Loro snapshot is the authority, and it
reused the same helper. After a power loss the rename can reach the disk
before the data does, which leaves a zero-length or old snapshot while SQL
and the org files already reflect the newer state. A process kill does not
show this; only a power loss or kernel panic does.

## Missing piece
No test or PBT transition cuts power or checks the order of sync calls: the
keystone and the Loro suite only kill or restart the process, and the page
cache hides a missing fsync. The `DurabilityRecorder` seam in `fs_port` now
records `FileSynced`, `Replaced` and `DirSynced`.

## Remedy
FIXED. `write_durable_blocking` writes a temp file, `sync_all`s it, renames
it, then syncs the directory. The snapshot write and the update-log reset use
it; `write_atomic_blocking` keeps its documented no-fsync contract for the org
mirror. Each log append ends in `sync_data`.

- Red: `crates/holon-loro/tests/snapshot_write_is_durable.rs`, log
  `lane-logs/dvul-i6-red-fsync.log` (only `Replaced` was recorded, no
  `FileSynced` or `DirSynced`).
- Green: `lane-logs/dvul-i6-green-1.log` and `dvul7-green-1.log`.
- Teeth: sabotage `non-durable-snapshot` and `no-log-fsync` both turn the
  tests red, files restored by sha256 (`lane-logs/dvul-i6-teeth.log`).
- Cost, profile `release` (`lane-logs/dvul-i6-fsync-cost-release.log`): one
  field save (append plus `sync_data`, which is a full fsync on macOS) has a
  median of 4.1 ms and a p95 of 4.6-6.0 ms at N=500 and N=8000 blocks; a
  snapshot save costs 27 ms (N=500) and 35 ms (N=8000).

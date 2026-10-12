---
id: 2026-10-12-db-open-locked-fails-boot-at-once
date: 2026-10-12
gap: ENVIRONMENT
secondary: null
status: PARTIAL
summary: >-
  A database file that another process held for a moment stopped the boot at once, because
  the open treated the first `Locked` error as final.
---

## Bug
Found by the honeymoon2 boot-ladder census (code audit, lane boot-ladder-gaps). Reproduced
red-first by `a_database_held_briefly_opens_once_released` in
`crates/holon-turso/tests/db_open_keeps_a_usable_file.rs`: a child process opened the file
while the parent held it for 1 s and got `CHILD-UNOPENABLE Locked`.

## Root cause
`crates/holon-turso/src/db_open.rs` `open` returned the typed `DatabaseOpenFailure::Locked`
error on the first try. A process that exits slowly (an old Holon, a backup tool) holds the
lock for a short time only.

## Missing piece
The keystone runs in one process, and the engine shares one `Database` per file in a
process, so no keystone or in-process test can reach a lock that another process holds.

## Remedy
The open tries again on `Locked` up to `LOCK_TRIES` times, `LOCK_RETRY` apart (5 s in all on
unix), with a `tracing::warn` for each wait. Then it returns the typed `Locked` error. No
condition is raised during the wait: the window does not exist yet, so nobody can see it.
Open: a lock held past the wait still stops the boot. The degraded no-Turso boot (BootLadder
rung 1a, Increment 4) is not built; see
`2026-10-12-unopenable-database-ends-the-boot`.

---
id: 2026-10-10-truncated-wal-loses-committed-rows-silently
date: 2026-10-10
gap: COVERAGE
secondary: ORACLE
status: OPEN
summary: >-
  A write-ahead log cut short (to 0 or 40 bytes, the crash shape) opens without an error,
  the committed rows that were only in the log are gone, and the open reports `Kept`.
---

## Bug
Found by the verifier of holon-work #155 (round 2, a probe of the production open). A healthy
database with committed rows still in its 28 872-byte log: after the log is truncated to 40
bytes or emptied, the open returns `Ok(Kept)` and `SELECT … FROM t` fails with "no such table".
Pinned by the ignored test `a_truncated_write_ahead_log_does_not_lose_rows_silently` in
`crates/holon-turso/tests/db_open_keeps_a_usable_file.rs`
(`docs/Testing/red-logs/2026-10-10-db-open-sidecar-wal-encoding-lock-RED.txt`).

## Root cause
The engine raises no error for a log shorter than its header or empty, so `db_open::open`
(`crates/holon-turso/src/db_open.rs`) cannot classify it; the lost frames are not detected.

## Missing piece
The corruption fault ladder truncates only the main file (`TursoCorruption::TruncateDbFile`,
`crates/holon-integration-tests/src/faults.rs`), and no invariant compares the rows a boot finds
with the rows committed before it.

## Remedy
Open: tracked as its own task under holon-work #155.

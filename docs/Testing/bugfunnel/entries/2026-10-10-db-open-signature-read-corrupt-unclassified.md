---
id: 2026-10-10-db-open-signature-read-corrupt-unclassified
date: 2026-10-10
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A damaged schema page that the engine reports only when the open reads the function-set
  table fails every boot with an untyped error, and nothing is moved aside.
---

## Bug
Found by the verifier of holon-work #155 (round 3, a probe of the production open). A database
with 80+ tables, checkpointed, then page 2 overwritten with `0xAB`: `Database::open` and the
first connect succeed, and `SELECT signature FROM holon_db_fn_set` raises
`Corrupt database: Invalid page type: 171`. The open returned
`DatabaseError("SELECT signature FROM holon_db_fn_set: …")`, and three boots in a row failed
the same way with the file still in place.

## Root cause
`crates/holon-turso/src/db_open.rs` classified engine errors only on the file open and the
first connect. The reads after them (the signature read, the view drop, the record write)
turned the `LimboError` into an untyped `StorageError::DatabaseError(String)`, so the
`Corrupt` class never reached the recovery.

## Missing piece
No test damaged a page that the open reads after the first connect. The keystone corruption
ladder (`TursoCorruption`, `crates/holon-integration-tests/src/faults.rs`) truncates the main
file only, and it accepts a typed boot error as a valid outcome, so it cannot tell a refused
boot from a recovered one.

## Remedy
Every engine call of the open now returns `OpenError::Engine` with its `LimboError`, and `open`
classifies the errors of every leg in one place (`Leg::Open`, `Leg::Connect`, `Leg::Use`).
Pinned by `a_damaged_page_the_open_reads_is_moved_aside` in
`crates/holon-turso/tests/db_open_keeps_a_usable_file.rs`
(`docs/Testing/red-logs/2026-10-10-db-open-signature-leg-RED.txt`).

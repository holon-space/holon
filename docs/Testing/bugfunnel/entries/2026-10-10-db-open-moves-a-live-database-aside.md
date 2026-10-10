---
id: 2026-10-10-db-open-moves-a-live-database-aside
date: 2026-10-10
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  A second process that opened the same database file moved the first process's LIVE
  database aside to `.unusable-<ts>`, because the open moved the file aside on every engine
  error, not only on corruption; a read-only database file was moved aside the same way.
---

## Bug
Found by the senior review of holon-work #155 (a code audit), reproduced red-first by
`crates/holon-turso/tests/db_open_keeps_a_usable_file.rs`
(`docs/Testing/red-logs/2026-10-10-db-open-moves-live-db-aside-RED.txt`): process B got
"File is locked by another process" from the engine and reported `MovedAside`, with process
A's file renamed under it. A database file without write permission ("permission denied" at
open) was moved aside too.

## Root cause
`crates/holon-turso/src/db_open.rs` `open` called `set_aside` on any `open_file` error.
`open_with_io` flattened the engine's typed `LimboError` into a `DatabaseError(String)`, so
the open could not tell a held lock or an I/O refusal from a damaged file. The database path
comes from the config directory, not the vault, so the vault lock does not stop a second
process.

## Missing piece
Every test opened a database from one process. The engine's lock is a per-process `fcntl`
lock and the engine shares one `Database` per file in a process, so no in-process test or
keystone reboot can reach the lock error.

## Remedy
`open_with_io` and the first connect parse the engine error at the boundary into
`StorageError::DatabaseUnopenable { failure: DatabaseOpenFailure, .. }` (`Locked`, `Io(kind)`,
`Corrupt`, `Unclassified`). `Locked` is only real contention: a lock another process holds, or a
database another process has open. `Corrupt` is `Corrupt`, `NotADB` and the short-read,
WAL-frame and checksum completion errors; it moves aside the smallest set of files that can
explain it: a damaged shared-WAL coordination file (`-tshm`) alone, then the write-ahead log
alone when the first connect raised the error (disclosed as `database-log-moved-aside`), else
every file of the database (`database-moved-aside`). Every other class keeps the files and
returns the typed error; boot discloses it as `BootError { Turso, EngineResolve }` with the
whole cause chain, so the report names the fix. The integration test runs a real second process.
Open: boot still stops on a `Locked` or `Io` open (degraded boot, #155).

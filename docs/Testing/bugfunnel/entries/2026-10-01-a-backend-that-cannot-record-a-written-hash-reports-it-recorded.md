---
id: 2026-10-01-a-backend-that-cannot-record-a-written-hash-reports-it-recorded
date: 2026-10-01
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  A block store that did not implement `record_file_hash` got a default that recorded nothing and returned success, so the hash stamp disclosed the written files as recorded.
---

## Bug
Found by review in D229 round 12 (agent 3, FLAG 2 in `lane-logs/d229-r12-report.md`), lane d229-move. The default `BlockReader::record_file_hash` was `Ok(())`. On a backend that used it, `stamp_written_hashes` raised `written_files_recorded` although no row held the hash.

## Root cause
The trait default in `crates/holon-filesystem/src/sync_ports.rs` copied the no-op pattern of `persist_file_projection`. A backend that forgot the method could not be told apart from one that recorded the hash.

## Missing piece
Only the Turso backend (`CacheBlockReader`) was exercised by the stamp tests. No test drove the stamp on a backend without the method.

## Remedy
The default now fails and names the backend type. `LoroBlockReader` (no `file` table) implements the method itself as a no-op, which it states. Test `crates/holon-orgmode/tests/written_hash_stamp_without_a_file_table.rs`: red before the fix (`lane-logs/d229r12d-stamp-red.log`, disclosures `["recorded"]`), green after (`lane-logs/d229r12d-stamp-green.log`), red with the default set back to `Ok(())` in place (`lane-logs/d229r12d-teeth-default.log`, restore sha-checked).

---
id: 2026-10-01-the-ingest-normalization-write-reaches-disk-before-its-hash-record
date: 2026-10-01
gap: COVERAGE
secondary: null
status: OPEN
summary: >-
  The ingest's normalization write still writes the org file before the `file` row records the hash, so a crash between the two has the same window D18.b closed for write-backs.
---

## Bug
Found by the D229 round-12 implementers (lane d229-move) by reading the code. After an ingest, the controller can write the file back in its rendered form (`crates/holon-filesystem/src/file_sync_controller.rs`, `self.fs.write(target, rendered)` followed by crash point `after_ingest_write_back`). That write does not go through `write_back_or_skip_readonly`, so the D18.b write-ahead of the empty hash does not cover it.

## Root cause
D18.b changed the write-back funnel only. The normalization write is a second disk-write site with its own order of write and record.

## Missing piece
No test kills the controller between the normalization write and its record and then restores the earlier bytes while closed (the shape of `a_restore_after_a_crash_before_the_written_hash_is_recorded_is_read_at_boot`, at crash point `after_ingest_write_back`).

## Remedy
Open. Route the normalization write through the same write-ahead, with a red-first crash test at `after_ingest_write_back`.

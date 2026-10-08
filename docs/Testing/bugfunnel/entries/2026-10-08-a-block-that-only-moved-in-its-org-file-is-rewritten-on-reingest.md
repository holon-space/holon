---
id: 2026-10-08-a-block-that-only-moved-in-its-org-file-is-rewritten-on-reingest
date: 2026-10-08
gap: ORACLE
secondary: null
status: FIXED
summary: >-
  Re-ingesting an org file in which blocks only changed position re-wrote every
  later block (121 writes and ~6.4 s for a move out of the 120-block
  Archive.org), and the read budget reported it as one read repeated 90x.
---

## Bug
Found by agent exploration in the "ingest batch read" lane, while chasing the
w20 landing-gate red row `MoveBlockBetweenFiles.sql_read_repeat` (keystone
known-red `move-block-between-files-sql-read-repeat-budget`). After the
per-block owner walk became one batch read, the move still showed
`SELECT "…" AS v FROM block_raw WHERE id = '…'` repeated 90x, 121 writes, and a
6.4 s settle against the 200 ms p95 SLO.

## Root cause
The org parser stamps `sequence`, a document-wide DFS counter, on every block
(`crates/holon-org-format/src/parser.rs:1234`). The org `content_differs`
compared it (`crates/holon-orgmode/src/file_format.rs:148` before the fix), so
removing or inserting one headline made every later block an "update" in the
ingest's update pass (`crates/holon-filesystem/src/file_sync_controller.rs`,
`ingest_adapter.content_differs(old_block, effective)`). In Loro mode
`update_in_tree` writes each param field with `set_field`
(`crates/holon/src/core/sql_block_operations.rs:913`); `content_type` is not a
Loro cell, so it fell through to the SQL `set_field`, which reads the old value
first (`read_field_old_value`, `crates/holon/src/core/sql_operation_provider.rs:2746`).
A probe of all 119 update-pass blocks of the move showed `sequence` as the ONLY
differing field. The `FileFormatAdapter::positional_property_keys` contract
(`crates/holon-core/src/file_format.rs:231`) already says positional keys are
not edits; `content_differs` disagreed with it.

The budget mis-described the defect: `read_field_old_value` inlines the id into
the SQL text, `redact_sql_for_logs` blanks same-length ids to the same text, and
the binding fingerprint hashed only bound params (`-` when none), so 90 reads
of 90 different rows counted as ONE binding repeated 90x.

## Missing piece
No test counted the writes a re-ingest issues for blocks the file did not
change, and the read-budget binding identity could not tell inlined values
apart.

## Remedy
`content_differs` no longer compares `sequence`; the ingest still places blocks
by document order through `place`/`place_all`. Pinned by
`crates/holon-orgmode/tests/reingest_writes_only_changed_blocks.rs`, whose
pure-reorder case asserts both zero writes and the new order in the store. The `query`/`execute` span
`params_fp` now digests the raw statement text with the bound params under the
process-keyed hasher (`crates/holon-turso/src/turso.rs`), so distinct inlined
ids are distinct bindings. Keystone replay of the w20 move: reads 714 → 44,
writes 121 → 2, wall 6.4 s → 0.7 s (hand-authored row
`w20-move-archive0-sourcefirst`). Open follow-up: `read_field_old_value` and
its siblings should bind the id as a parameter instead of inlining it.

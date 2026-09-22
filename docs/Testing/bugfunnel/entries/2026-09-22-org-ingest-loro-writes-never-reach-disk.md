---
id: 2026-09-22-org-ingest-loro-writes-never-reach-disk
date: 2026-09-22
gap: COVERAGE
secondary: ORACLE
status: FIXED
summary: >-
  Only the UI command path saved the Loro snapshot, so every Loro write that
  arrived through org ingest (and every write at all since the last UI op)
  existed only in memory: a crash or a clean quit rolled the Loro authority
  back behind SQL and the org files.
---

## Bug

Martin's live app: `holon_tree.loro` on disk was 9 hours old (13:03 CEST)
while the projection sidecar `holon_tree.loro.sync` was current (22:07). The
last dispatched UI op of the session was at 13:03:53 (same second as the
snapshot mtime); after it, 236 org-ingest passes (agent session logs under
`Agents/cc/`) wrote Loro and SQL and none saved. Found by a read-only log and
code investigation of the running app.

## Root cause

- The one save on the normal write path was `LoroBlockOperations::save_doc`
  (`crates/holon-loro/src/loro_block_operations.rs`, called after each block
  op). Org ingest goes `FileSyncController` → `apply_ingest_batch`
  (`crates/holon/src/core/sql_block_operations.rs:879`) → `BlockCellRegistry`
  writes into the same in-memory document and never saves.
- `holon_app::shutdown_session` stopped the session tasks and closed Turso but
  never saved Loro, so a clean quit lost the same writes as a crash.
- The projection wrote SQL and advanced the sidecar watermark on every pass
  regardless of what was on disk, so SQL was routinely ahead of the snapshot.

Measured in the keystone before the fix
(hand-authored `org-ingest-write-survives-reboot`): after `Reboot`,
`inv-blocks-match-ref/loro` reports `only in reference (1): ["block:bulk-1-0"]`
and the read-model check reports `block:bulk-1-0: loro_node=Never`; the boot
took the cold-boot fast path for the appended file (the recovery hole, see
`2026-09-22-stale-loro-snapshot-drops-blocks-on-boot`). With
`inv-loro-snapshot-covers-projection` wired, every Loro+Turso case goes red
at the initial state: `SQL is projected up to Frontiers([574@…]), the snapshot
… stops at VersionVector({…: 512})`.

## Missing piece

COVERAGE: no generated sequence reloads `.loro` from disk after an
ingest-only write. `SimulateRestart` only touches org files in a running
engine, and `Reboot` is off by default (D104.a). ORACLE: the harness saved in
the SUT's place before a restart (`persist_loro_snapshot` in
`crates/holon-integration-tests/src/faults.rs`; the loro_sync stub SUT's
`Restart`), and no invariant said "the snapshot on disk holds what SQL
reflects".

## Remedy

- Persistence belongs to the document store, not to one caller:
  `LoroDocumentStore::save_all` writes every document whose committed
  frontiers are not on disk yet (skip otherwise), serialized by one lock held
  across the save.
- Ordering rule: `LoroProjection::emit_ops` calls `save_all` before every SQL
  sink write and sidecar advance, so a crash can never leave SQL (or a
  `file.content_hash` stamped after the ingest flush) ahead of the snapshot.
  One save per projection pass coalesces all commits since the last pass.
- `shutdown_session` saves after the session tasks stop and before Turso
  closes.
- `LoroBlockOperations::save_doc` is removed; there is one save path.
- Oracle: `inv-loro-snapshot-covers-projection`
  (`crates/holon-loro-testing/src/invariants/loro_snapshot_covers_projection.rs`)
  over the new `SutLoroDurability` capability. Coverage: hand-authored keystone
  case `org-ingest-write-survives-reboot`; loro_suite
  `loro_snapshot_durability::an_ingested_block_is_on_disk_once_sql_shows_it`.
  The harness no longer saves for the SUT: `persist_loro_snapshot` is gone and
  the stub `Restart` is a crash at quiescence.
- Measured save cost (release): 3,330 blocks / 214k ops, one save after one
  edit p50 2.2 ms, p95 2.6 ms (full snapshot, 570 KB); 552k ops p50 4.2 ms,
  p95 5.7 ms; the every-64th compacting save 15–32 ms; a call with nothing to
  save < 4 µs.

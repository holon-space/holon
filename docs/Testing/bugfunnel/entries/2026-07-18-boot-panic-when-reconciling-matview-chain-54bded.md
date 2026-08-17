---
id: 2026-07-18-boot-panic-when-reconciling-matview-chain-54bded
date: 2026-07-18
gap: ENVIRONMENT
secondary: null
status: OPEN
summary: >-
  Boot PANIC when reconciling the `block` matview chain against a PERSISTED
  on-device DB (discovered while confirming the Android black-screen fix on
  the OnePlus DN2103, which still carried a `holon.db` from the stale
  2026-03-26 APK): `PANIC at crates/holon/src/di/schema_providers.rs:185:
  BlockMatviewView schema init failed: [block_matview] ensure_schema failed:
  Failed to execute DDL: Parse error: Cannot drop system table
  __turso_internal_dbsp_state_v1_events_view_block`.
  `BlockMatviewSchemaModule` reconcile issues `DROP TABLE IF EXISTS
  __turso_internal_dbsp_state_v1_events_view_block` (Turso IVM's own internal
  DBSP state table) and Turso rejects dropping a system table, aborting boot
  and ending the TursoBackend actor loop — the app never reaches window-open
  on a pre-existing DB. `pm clear` (fresh DB) boots cleanly, which is how the
  black-screen fix was then confirmed. Likely same family as the known
  matview-reopen cluster (BugFunnel 2026-07-13 B1 /
  `matview_reboot_duplicate_repro.rs`) but a distinct failure mode: a hard DDL
  panic during schema reconcile, not a silent row desync.
source_line: 1009
---

## Bug

Boot PANIC when reconciling the `block` matview chain against a PERSISTED
on-device DB (discovered while confirming the Android black-screen fix on
the OnePlus DN2103, which still carried a `holon.db` from the stale
2026-03-26 APK): `PANIC at crates/holon/src/di/schema_providers.rs:185:
BlockMatviewView schema init failed: [block_matview] ensure_schema failed:
Failed to execute DDL: Parse error: Cannot drop system table
__turso_internal_dbsp_state_v1_events_view_block`.
`BlockMatviewSchemaModule` reconcile issues `DROP TABLE IF EXISTS
__turso_internal_dbsp_state_v1_events_view_block` (Turso IVM's own internal
DBSP state table) and Turso rejects dropping a system table, aborting boot
and ending the TursoBackend actor loop — the app never reaches window-open
on a pre-existing DB. `pm clear` (fresh DB) boots cleanly, which is how the
black-screen fix was then confirmed. Likely same family as the known
matview-reopen cluster (BugFunnel 2026-07-13 B1 /
`matview_reboot_duplicate_repro.rs`) but a distinct failure mode: a hard DDL
panic during schema reconcile, not a silent row desync.

## Missing piece

the keystone boots a FRESH temp DB per case and never re-reconciles the
block-matview chain over a persisted DB carrying Turso
`__turso_internal_dbsp_state_*` tables, so a reconcile-time "cannot drop
system table" DROP is never exercised; no restart-with-persisted-DB boot
smoke test on any platform

## Remedy

OPEN — found on-device, not fixed here (out of scope for the black-screen
task); reconcile must not emit `DROP TABLE` against Turso-internal DBSP
state tables (skip/omit them, or reconcile via a supported matview-recreate
path). Repro: launch on a device whose `holon.db` predates the current
block-matview schema.

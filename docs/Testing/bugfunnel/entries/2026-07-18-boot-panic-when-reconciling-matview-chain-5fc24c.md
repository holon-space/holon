---
id: 2026-07-18-boot-panic-when-reconciling-matview-chain-5fc24c
date: 2026-07-18
gap: ENVIRONMENT
secondary: null
status: FIXED
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
source_line: 1006
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

FIXED 2026-07-18 (holon-side; turso pin untouched). ROOT CAUSE:
`reconcile_named_view`'s DBSP-state cleanup
(`matview_manager.rs::cleanup_orphaned_dbsp_state` + the
`MatviewManager::cleanup_orphaned_dbsp_tables` mirror) issued `DROP TABLE IF
EXISTS __turso_internal_dbsp_state_v%_{view}` — but Turso reserves the
`__turso_internal_` prefix (`schema.rs::is_system_table`,
`RESERVED_TABLE_PREFIXES`) and `validate_drop_table`
(`translate/schema.rs:2067`) bails `Cannot drop system table` whenever the
table actually EXISTS (the `IF EXISTS` early-return at :2093-2098 only skips
the guard when the table is ABSENT — so the cleanup was a no-op when there
was nothing to clean and fired the panic precisely when a real state table
was present, e.g. the stale-DBSP-epoch orphan on the persisted device DB).
The `v%_block` LIKE pattern also over-matched the sibling
`__turso_internal_dbsp_state_v1_events_view_block`. FIX: cleanup no longer
DROPs — disposal belongs to Turso itself
(`translate_create_materialized_view` reclaims a current-epoch orphaned
state table as part of (re)creation; `translate_drop_view` destroys it when
the matview row exists). A residual older-epoch state table carries a
different circuit-version name so the current CREATE never collides with it;
cleanup now DISCLOSES it via `tracing::warn!` (fail-loud, never silent) and
leaves it. Base tables (source of truth) are never touched → NO reseed
needed (satisfies the "full reseed takes minutes on-device = last resort"
constraint). Red→green guard:
`matview_manager::tests::cleanup_never_issues_forbidden_system_table_drop`
(reproduced the exact `Cannot drop system table
__turso_internal_dbsp_state_v1_src_view` panic, now Ok). ENVIRONMENT gap
CLOSED:
`matview_manager::tests::reconcile_over_persisted_db_survives_definition_change`
— the first persisted-DB (file-backed) boot rung: create matview → shutdown
→ reopen over the SAME file → reconcile a CHANGED definition (DROP+CREATE
over persisted DBSP state) without panic, base row intact. TURSO-SIDE
follow-up (NOT blocking, pin untouched): there is no user-facing SQL to
reclaim a cross-DBSP-circuit-version orphaned state table (DROP VIEW only
cleans the current-version name); proposed fork enhancement = have DROP VIEW
/ CREATE sweep `__turso_internal_dbsp_state_*_{view}` across ALL version
prefixes. Gates green: holon-turso lib 131/131, holon-advice matview_build
4/4, matview_reboot_duplicate_repro 2/2.

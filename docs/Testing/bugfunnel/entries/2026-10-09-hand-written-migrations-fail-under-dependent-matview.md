---
id: 2026-10-09-hand-written-migrations-fail-under-dependent-matview
date: 2026-10-09
gap: ENVIRONMENT
secondary: null
status: FIXED
summary: >-
  The hand-written ADD COLUMN migrations for block_raw, file and integration_state failed on a real database because the engine refuses ALTER TABLE under a stored materialized view.
---

## Bug
`add_missing_property_kinds_column`, `add_missing_read_only_blocks_column` and
`add_missing_integration_state_columns` ran `ALTER TABLE ... ADD COLUMN` on
tables that a stored matview reads (`block`, the sidebar watch view). The
engine refuses that ALTER, so the migration fails on every real database that
needs it. Found by code audit in lane stale-type.

## Root cause
The Turso fork refuses `ALTER TABLE` on a table with a dependent materialized
view. Each migration test seeded the old table without the matview that a real
database holds, so the tests were green.

## Missing piece
The migration tests did not reproduce the stored matview a real database
carries over the migrated table.

## Remedy
The three per-table migrations are replaced by one generic step:
`table_shape::reconcile_table` drops the dependent views, adds the declared
columns, and the views' owners recreate them. Pinned by
`crates/holon-turso/tests/column_added_under_dependent_matview.rs` (red log
`lane-logs/stale-type-red-handwritten.log` in the lane).

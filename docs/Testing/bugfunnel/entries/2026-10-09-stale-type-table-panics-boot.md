---
id: 2026-10-09-stale-type-table-panics-boot
date: 2026-10-09
gap: ENVIRONMENT
secondary: COVERAGE
status: FIXED
summary: >-
  A free-standing type table stored by an older binary with a different column shape panicked the whole boot instead of disabling only that type.
---

## Bug
A database whose `<type>_raw` table (for example `pantry_item_raw`) had an
older shape than the type's declaration aborted boot: `FreeStandingTypeViews`
panicked when `TursoAdapter::register` failed, and every core schema provider
used `.expect`. Found outside the automated tests while bringing a database
from an older build forward (lane stale-type).

## Root cause
`crates/holon/src/di/schema_providers.rs` turned every schema error into a
panic, and `CREATE TABLE IF NOT EXISTS` never compares a stored table with its
declaration. A declared column the stored table lacks, or a changed column
type, therefore reached the first write or the matview DDL and failed there.

## Missing piece
No test boots over a database an older binary created. Every PBT and
integration test starts from a fresh database, so the stored shape always
equals the declared shape.

## Remedy
`crates/holon-turso/src/table_shape.rs` compares each stored table with its
declaration by structure. A lossless difference adds the columns and raises
`table-columns-added`. A refused type table serves every other type, raises
`type-table-refused` with the `type_table.drop_refused_table` remedy
(`crates/holon-app/src/type_table_remedy.rs`), and makes writes to that type
fail with that reason. A failed schema module raises `schema-module-failed`
and boot continues. Pinned by `crates/holon-app/tests/stale_type_table_boots.rs`
(the red log is `lane-logs/stale-type-red-app.log` in the lane).

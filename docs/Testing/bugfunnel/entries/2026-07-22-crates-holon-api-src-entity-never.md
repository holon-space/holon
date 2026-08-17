---
id: 2026-07-22-crates-holon-api-src-entity-never
date: 2026-07-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `TypeDefinition::to_create_table_sql` (crates/holon-api/src/entity.rs) never
  quoted column (or PK-clause / index) identifiers, so any sidecar schema
  column named with a SQL keyword (`end`, `primary`, `order`, …) emits `CREATE
  TABLE … (end TEXT, …)` and Turso rejects it (`near "TEXT": syntax error`) at
  integration startup. Found by the gcal integration lane, which worked around
  it by renaming columns to `end_time`/`is_primary`.
source_line: 1100
---

## Bug

`TypeDefinition::to_create_table_sql` (crates/holon-api/src/entity.rs) never
quoted column (or PK-clause / index) identifiers, so any sidecar schema
column named with a SQL keyword (`end`, `primary`, `order`, …) emits `CREATE
TABLE … (end TEXT, …)` and Turso rejects it (`near "TEXT": syntax error`) at
integration startup. Found by the gcal integration lane, which worked around
it by renaming columns to `end_time`/`is_primary`.

## Missing piece

No test ever constructed a `TypeDefinition` with a SQL-keyword column name
and executed its generated DDL against a real engine; the column-name
alphabet in every existing test avoided keywords, so identifier quoting was
never exercised. (Keystone cannot reach it either — it generates no
sidecar/entity schemas with arbitrary keyword column names.)

## Remedy

FIXED 2026-07-22 — quoted table + column + PK-clause + index identifiers
with double quotes in `to_create_table_sql`/`to_index_sql`
(crates/holon-api/src/entity.rs). Red-first remedy test
`crates/holon-turso/tests/create_table_keyword_columns.rs` builds a
`TypeDefinition` with `end`/`primary`/`order` columns (inline PK + indexed,
and composite PK) and executes its DDL against a real in-memory Turso
(`TursoBackend::new_in_memory`) — RED (CREATE TABLE syntax error) before the
fix, GREEN after. Entity-unit exact-string assertions updated for the
quoting. KNOWN-ADJACENT (not fixed, flagged): the parallel
`generate_create_table_sql_with_change_origin` in
crates/holon/src/core/queryable_cache.rs has the same unquoted bug AND its
INSERT/SELECT use bare column names, so quoting only its CREATE would
relocate the break to insert-time — needs its own red-first test + wider
quoting pass. (NOW FIXED — see the QueryableCache change-origin row below.)

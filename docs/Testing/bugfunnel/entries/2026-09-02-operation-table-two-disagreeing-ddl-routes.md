---
id: 2026-09-02-operation-table-two-disagreeing-ddl-routes
date: 2026-09-02
gap: ORACLE
secondary: null
status: NOTED
summary: >-
  The `operation` table is created twice at boot by two DDL routes of different
  widths, and nothing asserts they agree.
---

## Bug

`operation` has two competing `CREATE TABLE IF NOT EXISTS` routes and both run
on every boot, in this order:

1. `OperationsSchemaModule` executes `crates/holon-turso/sql/schema/operations.sql`,
   which declares `_change_origin`. This is the statement that really creates
   the table.
2. `OperationLogStore::initialize_schema`
   (`crates/holon/src/core/operation_log.rs:50-57`) runs
   `TypeDefinition::to_create_table_sql()` for the same entity, which does NOT
   declare `_change_origin`. Against the existing table it is a no-op.

So the entity's own type definition and the schema file disagree about the
table's shape, and which one a reader believes decides whether write
provenance is thought to exist on that relation. Found by the fresh-context
verifier of the `change-origin-schema` lane (D67.a) while probing what the
schema catalog reports for each relation — a code audit plus a live
`PRAGMA table_info` probe on a full DI boot, not by any test.

## Root cause

Two independent declarations of one relation, with no link between them:

- `crates/holon-turso/sql/schema/operations.sql:10` — `_change_origin TEXT`.
- `crates/holon-api/src/entity.rs:640` `to_create_table_sql` emits exactly the
  `TypeDefinition`'s fields, and `OperationLogEntry`'s definition names no
  `_change_origin`.

`block_raw` is protected against exactly this drift by
`crates/holon-turso/tests/schema_source_ddl_lock.rs`, which locks the schema
declaration to the DDL in both directions. `operation` has no such lock, so
the two routes were free to diverge.

## Missing piece

No invariant asserts that a relation has ONE authoritative shape. The DDL lock
exists for `block_raw` alone; nothing generalises it to every relation that is
both declared as a `TypeDefinition` and created by a schema file. A boot runs
both routes in every test wiring, so the interaction is generated on every
run — only the oracle is absent.

## Remedy

Not fixed here: the D67.a lane replaced the hardcoded `TABLES_WITH_CHANGE_ORIGIN`
list with a schema catalog that reports what the ENGINE has, so the catalog now
answers correctly (`_change_origin` present) whichever route ran last, and
`crates/holon-turso/tests/schema_catalog_boot.rs::a_narrow_no_op_create_cannot_unsay_a_column_the_engine_has`
pins that. The underlying product inconsistency stands.

Open: generalise the `schema_source_ddl_lock` idea so every relation with both
a `TypeDefinition` and a schema-file DDL is locked to one shape, then reconcile
`operation`'s two routes.

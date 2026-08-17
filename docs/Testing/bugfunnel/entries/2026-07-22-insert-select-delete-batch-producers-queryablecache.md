---
id: 2026-07-22-insert-select-delete-batch-producers-queryablecache
date: 2026-07-22
gap: COVERAGE
secondary: null
status: FIXED
summary: >-
  `generate_create_table_sql_with_change_origin` and the INSERT / SELECT /
  DELETE / batch producers in the QueryableCache change-origin path
  (crates/holon/src/core/queryable_cache.rs) never quoted table or column
  identifiers, so a cache whose schema has a SQL-keyword column (`end`,
  `order`, `primary`, …) breaks at CREATE TABLE (`near "order": syntax
  error`), and — because the single-row upsert, `apply_batch` INSERT/ON
  CONFLICT/`excluded.*`, SELECT-by-id, SELECT-*, `get_all_ids`, `clear`, and
  delete all interpolated bare column/table names — quoting only the CREATE
  would merely relocate the break to insert-time. Sibling of the
  `to_create_table_sql` row two rows above (row for entity.rs), where it was
  flagged KNOWN-ADJACENT.
source_line: 1106
---

## Bug

`generate_create_table_sql_with_change_origin` and the INSERT / SELECT /
DELETE / batch producers in the QueryableCache change-origin path
(crates/holon/src/core/queryable_cache.rs) never quoted table or column
identifiers, so a cache whose schema has a SQL-keyword column (`end`,
`order`, `primary`, …) breaks at CREATE TABLE (`near "order": syntax
error`), and — because the single-row upsert, `apply_batch` INSERT/ON
CONFLICT/`excluded.*`, SELECT-by-id, SELECT-*, `get_all_ids`, `clear`, and
delete all interpolated bare column/table names — quoting only the CREATE
would merely relocate the break to insert-time. Sibling of the
`to_create_table_sql` row two rows above (row for entity.rs), where it was
flagged KNOWN-ADJACENT.

## Missing piece

No test ever built a `QueryableCache` / `TypeDefinition` with a SQL-keyword
column name and drove its generated SQL against a real engine — every
existing cache test (`test_upsert_and_retrieve`, `test_apply_batch`, …) used
a keyword-free column alphabet (`id`/`title`/`priority`), so identifier
quoting on this path was never exercised. (Keystone cannot reach it either —
it generates no cache/entity schemas with arbitrary keyword column names.)

## Remedy

FIXED 2026-07-22 — a single module-level `q()` identifier-quoter now
double-quotes every identifier the path interpolates: CREATE (table + each
column + `_change_origin` + table-level `PRIMARY KEY (…)` clause),
single-row upsert (columns + `ON CONFLICT(id)` + `excluded.*` update
clause), `build_batch_statements` (INSERT OR IGNORE / upsert / delete),
SELECT-by-id, SELECT-*, `get_all_ids`, and `clear`. Red-first remedy test
`keyword_named_columns_work_end_to_end` (queryable_cache.rs `tests`) builds
a `KeywordRow` cache with `end` (indexed) + `order` columns and drives the
WHOLE path — CREATE+INDEX (cache construction) → single upsert → `get_by_id`
→ `apply_batch` INSERT → `get_all` → delete — against a real Turso
(`create_test_db_handle`): RED before the fix (`near "order": syntax error`
at CREATE, queryable_cache.rs:1473), GREEN after. No exact-string SQL
assertions existed on this path to update (the `backend_engine.rs` /
`traits.rs` string asserts are on other builders and are unaffected).

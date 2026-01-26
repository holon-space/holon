# Turso IVM Bug: Qualified `table.*` in SELECT breaks WHERE column resolution

## Bug Summary

When creating a materialized view with `SELECT alias.*` (qualified star from one specific table) in a multi-table join, Turso IVM misattributes all WHERE clause column references to the `*`-expanded table, even when they are explicitly table-qualified to a different table.

## Minimal Reproducer

```sql
CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, name TEXT, parent_id TEXT);
CREATE TABLE IF NOT EXISTS focus (region TEXT PRIMARY KEY, item_id TEXT);

-- PASSES: unqualified SELECT *
CREATE MATERIALIZED VIEW IF NOT EXISTS pass1 AS
SELECT * FROM focus AS f JOIN items AS b ON b.id = f.item_id WHERE f.region = 'main';

-- PASSES: explicit column list
CREATE MATERIALIZED VIEW IF NOT EXISTS pass2 AS
SELECT b.id, b.name, b.parent_id FROM focus AS f JOIN items AS b ON b.id = f.item_id WHERE f.region = 'main';

-- FAILS: qualified star from one table
CREATE MATERIALIZED VIEW IF NOT EXISTS fail1 AS
SELECT b.* FROM focus AS f JOIN items AS b ON b.id = f.item_id WHERE f.region = 'main';
-- Error: Parse error: Column 'region' with table Some("b") not found in schema
```

## Expected Behavior

All three queries should succeed. `SELECT b.*` is standard SQL and `WHERE f.region` is explicitly qualified to table `f` (focus), not `b` (items).

## Actual Behavior

IVM's column resolver expands `b.*` to the columns of `items`, then incorrectly resolves `f.region` against table `b` (items) instead of `f` (focus). Since `items` has no `region` column, it fails.

The error message confirms the misattribution: `Column 'region' with table Some("b")` — it should say `Some("f")` or simply succeed.

## Quoting does not matter

Both `f."region"` and `f.region` produce the same error.

## Impact

Any SQL generator that uses `RETURN node` semantics (GQL-to-SQL compilers, graph query engines) naturally emits `SELECT target.*` to return all columns of a specific node. This pattern is unusable in materialized views.

## Workaround

Replace `alias.*` with an explicit list of all columns from that table. Requires knowing the schema at SQL generation time.

## Where to Look in Turso

The IVM DDL parser that resolves column references in WHERE clauses. When the SELECT list contains a qualified star (`b.*`), the column resolution pass appears to bind all subsequent unresolved column references to that table, ignoring explicit table qualifiers on the column references.

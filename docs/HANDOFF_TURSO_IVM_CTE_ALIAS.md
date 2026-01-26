# Turso IVM Bug: Aliasing a CTE in FROM clause breaks join column resolution

## Bug Summary

When creating a materialized view, aliasing a CTE (recursive or non-recursive) in the FROM clause causes Turso IVM to fail with `Parse error: Join condition column 'X' not found in either input`. The same query works without the alias.

## Minimal Reproducer

```sql
-- Table setup (any table works)
CREATE TABLE IF NOT EXISTS items (id TEXT PRIMARY KEY, parent_id TEXT);

-- PASSES: CTE without alias
CREATE MATERIALIZED VIEW IF NOT EXISTS pass AS
WITH RECURSIVE r AS (
  SELECT id AS nid, 0 AS d FROM items
  UNION ALL
  SELECT b.id, r.d+1 FROM r JOIN items b ON b.parent_id = r.nid WHERE r.d < 2
)
SELECT b.id FROM r JOIN items b ON b.id = r.nid;

-- FAILS: identical query, but CTE aliased as 'x'
CREATE MATERIALIZED VIEW IF NOT EXISTS fail AS
WITH RECURSIVE r AS (
  SELECT id AS nid, 0 AS d FROM items
  UNION ALL
  SELECT b.id, r.d+1 FROM r JOIN items b ON b.parent_id = r.nid WHERE r.d < 2
)
SELECT b.id FROM r x JOIN items b ON b.id = x.nid;
-- Error: Parse error: Join condition column 'nid' not found in either input
```

## Also affects non-recursive CTEs

```sql
-- FAILS
CREATE MATERIALIZED VIEW IF NOT EXISTS fail2 AS
WITH r AS (SELECT id AS nid FROM items)
SELECT b.id FROM r x JOIN items b ON b.id = x.nid;
-- Same error
```

## Expected Behavior

Both queries should succeed — `r x` is standard SQL for aliasing a table/CTE in FROM and should resolve `x.nid` to the CTE's `nid` column.

## Actual Behavior

IVM's join condition parser does not resolve columns through CTE aliases. It only recognizes the CTE's original name.

## Impact

Any SQL generator that aliases CTEs (common in GQL-to-SQL compilers, ORMs, query builders) cannot produce materialized views with CTE joins.

## Workaround

Remove the alias from the CTE reference in the FROM clause: use `JOIN r ON r.col` instead of `JOIN r alias ON alias.col`. This requires control over the SQL generation layer.

## Where to Look in Turso

The IVM DDL parser that analyzes join conditions for incremental view maintenance. When it encounters `JOIN r x ON x.nid = ...`, it needs to resolve `x` → CTE `r` → column `nid`. Currently it appears to only check base table schemas and the CTE's original name, not aliases.

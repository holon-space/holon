# Handoff: GQL Variable-Length Path Query Crashes in execute_query

## Context

The Main Panel in `holon-pkm/index.org` now uses a GQL query to show the full document tree:

```gql
MATCH (root:Block)<-[:CHILD_OF*1..20]-(d:Block)
WHERE root.parent_id = 'holon-doc://7fe10dd7-2474-4ff9-ac17-523e39d90a24'
RETURN d.id, d.parent_id, d.content, d.content_type, d.source_language
```

The FK varlen path enhancement in `gql-to-sql` is already implemented. The query compiles correctly to a recursive CTE using `blocks.parent_id` (not the EAV `edges` table).

## The Bug

The compiled SQL **works** via `execute_raw_sql` but **crashes** via `execute_query` with:

```
Database error: Actor response channel closed
```

## What Works vs What Doesn't

| Method | Result |
|--------|--------|
| `mcp__holon-live__compile_query` (GQL) | Correct SQL output |
| `mcp__holon-live__execute_raw_sql` (same SQL) | Returns rows correctly |
| `mcp__holon-live__execute_query` (GQL) | **Crash**: Actor response channel closed |

## Compiled SQL (post-transform)

```sql
WITH RECURSIVE _vl1 AS (
  SELECT _v0.id AS node_id, 0 AS depth, CAST(_v0.id AS TEXT) AS visited
  UNION ALL
  SELECT _fk.id, _vl1.depth + 1, _vl1.visited || ',' || CAST(_fk.id AS TEXT)
  FROM _vl1
  JOIN blocks _fk ON _fk.parent_id = _vl1.node_id   -- FK edge (correct!)
  WHERE _vl1.depth < 20
  AND ',' || _vl1.visited || ',' NOT LIKE '%,' || CAST(_fk.id AS TEXT) || ',%'
)
SELECT _v2."id" AS "d.id", _v2."parent_id" AS "d.parent_id",
       _v2."content" AS "d.content", _v2."content_type" AS "d.content_type",
       _v2."source_language" AS "d.source_language",
       'blocks' AS entity_name, _v0._change_origin AS _change_origin
FROM blocks AS _v0
JOIN blocks AS _v2 ON 1 = 1       -- cross join (filtered by WHERE)
WHERE _v2.id IN (SELECT node_id FROM _vl1 WHERE depth >= 1 AND depth <= 20)
  AND _v0."parent_id" = 'holon-doc://7fe10dd7-2474-4ff9-ac17-523e39d90a24'
```

## Hypotheses (ordered by probability)

### 1. Parameter binding corrupts the SQL (MOST LIKELY)

`execute_query` calls `bind_context_params()` which adds `$context_id`, `$context_parent_id`, `$context_path_prefix` parameters, then `inline_parameters()` substitutes them into the SQL. Even though the GQL query doesn't use these params, `inline_parameters` might be corrupting the SQL — e.g., replacing `$` inside string literals or mangling the recursive CTE.

**How to verify**: Add logging in `execute_query` (backend_engine.rs:672) to print the SQL AFTER `inline_parameters` runs, and compare with the `compile_query` output.

**Files**: `crates/holon/src/api/backend_engine.rs` — `execute_query()`, `bind_context_params()`, `inline_parameters()`

### 2. Turso actor crashes on the cross join + recursive CTE

The `ON 1 = 1` cross join between `_v0` and `_v2` might cause the Turso actor to crash even though SQLite handles it fine. The Turso connection pool actor dying would explain "Actor response channel closed".

**How to verify**: Run the exact compiled SQL (including `_change_origin`) via `execute_raw_sql`. If it works, the issue is in the parameter/query pipeline, not Turso.

**Already verified**: The exact SQL works via `execute_raw_sql`, so this is less likely.

### 3. The `_change_origin` column reference uses wrong alias

The transform picks `_v0._change_origin` (the root node) when it should probably use `_v2._change_origin` (the result node). This might cause issues if the query pipeline expects `_change_origin` to correspond to the result rows. Probably not the crash cause but worth fixing.

**File**: `crates/holon/src/storage/sql_parser.rs` — `inject_change_origin_into_set_expr()` picks the first FROM table's alias.

## Fixes Already Applied in This Session

### 1. `_change_origin` alias fix (sql_parser.rs)

When GQL generates `FROM blocks AS _v0`, the `_change_origin` injector now uses the alias (`_v0._change_origin`) instead of bare table name (`blocks._change_origin`). Test: `test_change_origin_uses_alias`.

### 2. `_change_origin` matview skip (sql_parser.rs)

Added `TABLES_WITH_CHANGE_ORIGIN` allowlist so `_change_origin` is only injected for known base tables (`blocks`, `documents`, `directories`, `files`, `operations`, `todoist_tasks`, `todoist_projects`). Materialized views like `blocks_with_paths` are skipped. Test: `test_change_origin_skips_matview`.

### 3. GQL FK varlen paths (gql-to-sql, already done by user)

Variable-length paths now use `ForeignKeyEdgeResolver` when available, generating `JOIN blocks _fk ON _fk.parent_id = _vl1.node_id` instead of `JOIN edges e ON ...`.

## Secondary Issue: Navigation Awareness

The current GQL query hardcodes the document URI. To make it navigation-aware (change when the user clicks a different document in the sidebar), it would need to join with `navigation_cursor`/`navigation_history` tables. GQL can't express relational joins with non-graph tables.

Options:
- Add `navigation_cursor`/`navigation_history` as GQL node types in the GraphSchema
- Use a GQL parameter (`WHERE root.parent_id = :focused_doc`) filled from navigation context
- Use SQL/PRQL wrapper that joins navigation → passes doc URI as param to GQL subquery

## Key Files

| File | What |
|------|------|
| `holon-pkm/index.org:83` | Main Panel GQL query |
| `crates/holon/src/api/backend_engine.rs:672` | `execute_query()` — where crash occurs |
| `crates/holon/src/api/backend_engine.rs:618` | `bind_context_params()` — adds $context params |
| `crates/holon/src/storage/sql_parser.rs` | SQL transforms (entity_name, _change_origin) |
| `frontends/mcp/src/tools.rs:376` | MCP execute_query tool |
| `bigdata/gql-to-sql/crates/gql-transform/src/transform_match.rs:483` | FK varlen CTE generation |

## Reproduction

```
# Works:
mcp__holon-live__execute_raw_sql(sql: "<compiled SQL above>")

# Crashes:
mcp__holon-live__execute_query(
  query: "MATCH (root:Block)<-[:CHILD_OF*1..3]-(d:Block) WHERE root.id = '1365019b-48fa-4217-8691-8a8b9eba0fc3' RETURN d.id, d.parent_id, d.content LIMIT 5",
  language: "gql"
)
```

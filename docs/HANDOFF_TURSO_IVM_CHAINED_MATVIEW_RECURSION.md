# Turso IVM: Chained Matview Recursion Bugs

## Context

We're building a unified tree matview that combines `blocks` and `todoist_tasks` via UNION ALL, then computes hierarchical paths via recursive CTE. This is the foundation for embedding third-party elements into the block tree.

The approach: a **flat UNION ALL matview** (projecting both tables into common columns) feeding a **recursive CTE matview** that computes paths. Both layers need CDC for live updates.

## Three Bugs Found

All discovered on a live Turso instance with ~300 blocks and ~370 todoist_tasks.

### Bug 1: Recursive CTE over UNION matview only recurses 1 level

**Setup:**
```sql
-- Upstream: flat UNION ALL matview
CREATE MATERIALIZED VIEW unified_items AS
SELECT id, parent_id, content AS title, 'blocks' AS entity_name FROM blocks
UNION ALL
SELECT id, parent_id, content AS title, 'todoist_tasks' AS entity_name FROM todoist_tasks;

-- Downstream: recursive CTE matview reading from it
CREATE MATERIALIZED VIEW unified_tree AS
WITH RECURSIVE paths AS (
    SELECT u.id, u.parent_id, u.title, u.entity_name,
           '/' || u.id AS path, 0 AS depth
    FROM unified_items u
    WHERE u.parent_id IS NULL

    UNION ALL

    SELECT c.id, c.parent_id, c.title, c.entity_name,
           p.path || '/' || c.id, p.depth + 1
    FROM unified_items c
    JOIN paths p ON c.parent_id = p.id
    WHERE p.depth < 20
)
SELECT * FROM paths;
```

**Expected:** 3 depth levels for todoist_tasks (250 roots → 100 subtasks → 20 sub-subtasks), plus blocks at depth 1 (annotation blocks parented under todoist tasks).

**Actual:**
```
depth 0: 250 todoist_tasks (roots) ✓
depth 1: 100 todoist_tasks (subtasks) + 30 blocks (annotations) ✓
depth 2: 0 rows ✗  — 20 sub-subtasks missing
```

Recursion stops after exactly 1 recursive step. The `WHERE p.depth < 20` is not the limiting factor (verified depth 1 rows exist and have valid parent-child links to depth 2 data).

**Control:** The same recursive CTE structure works correctly when the source is a plain table (not a matview). In an earlier test with `_test_ext_items` (a regular table), a recursive CTE matview over a UNION ALL matview produced all expected depths. The difference may be data volume or specific column types.

### Bug 2: LIKE filter in base case silently drops rows

**Setup:** Same upstream `unified_items` matview as Bug 1. Different base case:

```sql
CREATE MATERIALIZED VIEW unified_tree AS
WITH RECURSIVE paths AS (
    SELECT u.id, u.parent_id, u.title, u.entity_name,
           '/' || u.id AS path, 0 AS depth
    FROM unified_items u
    WHERE u.parent_id IS NULL
       OR u.parent_id LIKE 'doc:%'    -- ← THIS DOESN'T MATCH
       OR u.parent_id LIKE 'sentinel:%'

    UNION ALL

    SELECT c.id, c.parent_id, c.title, c.entity_name,
           p.path || '/' || c.id, p.depth + 1
    FROM unified_items c
    JOIN paths p ON c.parent_id = p.id
    WHERE p.depth < 20
)
SELECT * FROM paths;
```

**Expected:** 12 blocks with `parent_id LIKE 'doc:%'` should appear at depth 0 (verified: `SELECT COUNT(*) FROM unified_items WHERE parent_id LIKE 'doc:%'` returns 12).

**Actual:** 0 blocks at depth 0. Only todoist tasks (matched by `parent_id IS NULL`) appear. The `LIKE 'doc:%'` filter silently produces no matches when used inside a recursive CTE matview that reads from an upstream UNION matview.

**Control:** `SELECT ... FROM unified_items WHERE parent_id LIKE 'doc:%'` works fine as a standalone query on the upstream matview (returns 12 rows). The LIKE only fails when used as the base case of a downstream recursive CTE matview.

### Bug 3: Simple recursive counter matview produces only 1 row

```sql
CREATE MATERIALIZED VIEW gen_numbers AS
WITH RECURSIVE gen(n) AS (
  SELECT 1 UNION ALL SELECT n+1 FROM gen WHERE n < 50
)
SELECT n FROM gen;

SELECT COUNT(*) FROM gen_numbers;
-- Expected: 50
-- Actual: 1
```

This is probably the simplest reproducer of the underlying issue: recursive CTE expansion in matviews doesn't iterate. The base case (SELECT 1) produces 1 row and the recursive step never fires.

### Bug 4 (from earlier HANDOFF): Inline subquery UNION in recursive step fails

```sql
CREATE MATERIALIZED VIEW tree AS
WITH RECURSIVE paths AS (
    SELECT id, parent_id, title, entity_name, '/' || id AS path, 0 AS depth
    FROM (
        SELECT id, parent_id, content AS title, 'blocks' AS entity_name FROM blocks
        UNION ALL
        SELECT id, parent_id, content AS title, 'todoist_tasks' AS entity_name FROM todoist_tasks
    ) AS all_items
    WHERE parent_id IS NULL

    UNION ALL

    SELECT c.id, c.parent_id, c.title, c.entity_name,
           p.path || '/' || c.id, p.depth + 1
    FROM (
        SELECT id, parent_id, content AS title, 'blocks' AS entity_name FROM blocks
        UNION ALL
        SELECT id, parent_id, content AS title, 'todoist_tasks' AS entity_name FROM todoist_tasks
    ) AS c
    JOIN paths p ON c.parent_id = p.id
    WHERE p.depth < 20
)
SELECT * FROM paths;
```

**Error:** `Join condition column 'parent_id' not found in either input`

The IVM join condition resolver can't see through subquery aliases to find columns. This forces the two-matview (chained) approach, which triggers Bug 1 and Bug 2.

## Impact

These bugs block a key architectural pattern: building a unified tree matview that combines multiple tables. The use case is embedding external system data (Todoist tasks, Jira issues, etc.) into the block tree for cross-boundary parent-child relationships and unified path computation.

### What Works (workarounds)

- UNION ALL matviews alone: work fine, CDC-stable
- Recursive CTE matviews over a **single plain table**: work fine
- Recursive CTE matviews over a **single non-UNION matview** (e.g., a simple JOIN matview): work fine
- The current `blocks_with_paths` matview (recursive CTE over `blocks` table directly): works fine

### What Doesn't Work

- Recursive CTE matview reading from upstream UNION ALL matview: recursion depth capped at 1
- LIKE filters in recursive CTE base case when source is upstream matview: silently drops rows
- Inline subquery UNIONs in recursive CTE matview: column resolution fails
- Simple self-referencing recursive counter matview: doesn't iterate

## Suggested Investigation Path

1. **Bug 3 first** — simplest reproducer, likely same root cause as Bug 1
2. Compare IVM plan for `WITH RECURSIVE gen(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM gen WHERE n < 50)` as:
   - Regular query (works) vs matview (broken)
3. For Bug 1, compare IVM plan when source is a table vs when source is a matview
4. The weight-tracking / delta-propagation logic for matview→matview CDC likely doesn't correctly handle recursive expansion

## Test Data on Live Instance

The following test artifacts may still exist on the live instance (clean up with DROP):
- `_perf_unified_items` matview
- `_perf_unified_tree` matview (may already be dropped)
- `_digits` table (helper for data generation)
- 370 rows in `todoist_tasks` (synthetic test data)
- 5 rows in `todoist_projects` (synthetic)
- 30 annotation blocks in `blocks` with `parent_id` pointing at todoist task IDs

## Related

- `docs/HANDOFF_TURSO_IVM_RECURSIVE_CTE_OVER_UNION_MATVIEW.md` — crash/corruption variant (recursive CTE over UNION matview causes "Actor response channel closed" or negative weight corruption)
- `docs/HANDOFF_TURSO_IVM_NEGATIVE_WEIGHT.md` — the negative weight IVM bug
- `crates/holon/src/storage/schema_modules.rs` — `blocks_with_paths` and `focus_roots` matview definitions

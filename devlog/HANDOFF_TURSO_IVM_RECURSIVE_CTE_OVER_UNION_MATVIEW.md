# Turso IVM Bug: Recursive CTE matview over UNION ALL matview crashes or corrupts

## Bug Summary

When a materialized view with a recursive CTE reads from an upstream UNION ALL materialized view, Turso either:
1. **Crashes** on first SELECT (actor channel closed), or
2. **Corrupts** IVM internal state during CDC propagation (`expected a positive weight, found -1`)

Both symptoms have been observed in the same session.

## Minimal Reproducer

```sql
-- Setup
CREATE TABLE items (id TEXT PRIMARY KEY, name TEXT, parent_id TEXT);
CREATE TABLE focus (region TEXT PRIMARY KEY, target_id TEXT);

INSERT INTO items VALUES
  ('a', 'Alpha', 'doc:1'),
  ('b', 'Beta', 'doc:1'),
  ('a1', 'Child-A1', 'a'),
  ('a2', 'Child-A2', 'a'),
  ('b1', 'Child-B1', 'b');
INSERT INTO focus VALUES ('main', 'doc:1');

-- Step 1: UNION ALL matview (resolves focus target to item IDs)
CREATE MATERIALIZED VIEW roots AS
SELECT f.region, f.target_id, i.id AS root_id
FROM focus AS f JOIN items AS i ON i.parent_id = f.target_id
UNION ALL
SELECT f.region, f.target_id, i.id AS root_id
FROM focus AS f JOIN items AS i ON i.id = f.target_id;

-- Verify: works fine
SELECT * FROM roots;
-- Returns: (main, doc:1, a), (main, doc:1, b)

-- Step 2: Recursive CTE matview that reads from the UNION matview
CREATE MATERIALIZED VIEW descendants AS
WITH RECURSIVE tree AS (
  SELECT i.id AS node_id, i.id AS source_id, 0 AS depth
  FROM items AS i
  UNION ALL
  SELECT child.id, tree.source_id, tree.depth + 1
  FROM tree JOIN items child ON child.parent_id = tree.node_id
  WHERE tree.depth < 10
)
SELECT d.*
FROM roots AS r
JOIN items AS root ON root.id = r.root_id
JOIN tree ON tree.source_id = root.id
JOIN items AS d ON d.id = tree.node_id
WHERE r.region = 'main';

-- CRASHES: Actor response channel closed
SELECT * FROM descendants;
```

## Observed Behaviors

### Crash on first SELECT (repro above)
```
Database error: Actor response channel closed
```

### Negative weight corruption (production, same query structure)
Matview creation succeeds, initial query may work, but after CDC propagation (e.g., block inserts/updates from org file sync):
```
Internal error: Invalid data in materialized view: expected a positive weight, found -1
```

After `DROP VIEW` + recreate, the matview returns correct data — until the next CDC update corrupts it again.

## What Works

- UNION ALL matview alone: works fine, queryable, CDC-stable
- Recursive CTE matview over a plain table: works fine
- Recursive CTE matview over a non-UNION matview: works fine
- The combination (recursive CTE over UNION ALL matview) crashes or corrupts

## Impact

This blocks using materialized view composition for navigation-aware queries. The use case is: a UNION matview resolves "focus target" (doc URI or block ID) into root block IDs, and a downstream recursive CTE matview expands those roots into full subtrees for rendering.

## Workaround

None known that preserves full CDC reactivity. Possible alternatives:
- Flatten the two matviews into a single SQL query (duplicating the UNION logic inline in the recursive CTE matview)
- Use a non-materialized view for the UNION step (loses CDC on the intermediate step)

## Where to Look in Turso

The IVM weight-tracking logic for matviews that depend on other matviews, specifically when the upstream matview uses UNION ALL. The weight bookkeeping may be double-counting or sign-inverting rows that come from UNION branches when propagating changes to downstream matviews.

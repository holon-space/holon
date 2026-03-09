# Turso IVM Bug: DROP+CREATE of upstream matview breaks DBSP chain to downstream matviews

## Summary

When a matview is DROPped and re-CREATEd, downstream matviews that were auto-loaded from `sqlite_master` (not explicitly re-created in the current session) lose their DBSP dependency on the recreated matview. CDC events stop cascading to those downstream matviews.

## Impact

Any app that:
1. Persists a database across sessions (matviews survive in `sqlite_master`)
2. Recreates some upstream matviews on startup (schema migration, idempotent setup)
3. Reuses downstream matviews without re-creating them

...will have broken CDC for the downstream matviews. The matview data becomes stale — queries against it return old results.

## Reproducer (pure SQL, no app logic)

```
Session 1 — fresh database:

CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER);
CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, region TEXT, block_id TEXT);
CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '', content_type TEXT DEFAULT 'text',
                    parent_id TEXT DEFAULT '', properties TEXT DEFAULT '{}');

INSERT INTO navigation_cursor VALUES ('main', NULL);

-- Matview chain: navigation_cursor → current_focus → focus_roots → watch_view
CREATE MATERIALIZED VIEW current_focus AS
  SELECT nc.region, nh.block_id
  FROM navigation_cursor nc JOIN navigation_history nh ON nc.history_id = nh.id;

CREATE MATERIALIZED VIEW focus_roots AS
  SELECT cf.region, cf.block_id, b.id AS root_id
  FROM current_focus cf JOIN block b ON b.parent_id = cf.block_id
  UNION ALL
  SELECT cf.region, cf.block_id, b.id AS root_id
  FROM current_focus cf JOIN block b ON b.id = cf.block_id;

CREATE MATERIALIZED VIEW watch_view AS
  WITH RECURSIVE _vl2 AS (
    SELECT _v1.id AS node_id, _v1.id AS source_id, 0 AS depth, CAST(_v1.id AS TEXT) AS visited
    FROM block AS _v1
    UNION ALL
    SELECT _fk.id, _vl2.source_id, _vl2.depth + 1, _vl2.visited || ',' || CAST(_fk.id AS TEXT)
    FROM _vl2 JOIN block _fk ON _fk.parent_id = _vl2.node_id
    WHERE _vl2.depth < 20
  )
  SELECT _v3.*, json_extract(_v3."properties", '$.sequence') AS "sequence"
  FROM focus_roots AS _v0
  JOIN block AS _v1 ON _v1."id" = _v0."root_id"
  JOIN _vl2 ON _vl2.source_id = _v1.id
  JOIN block AS _v3 ON _v3.id = _vl2.node_id
  WHERE _v0."region" = 'main' AND _v3."content_type" != 'source'
    AND _vl2.depth >= 0 AND _vl2.depth <= 20;

-- Seed data: two documents with children
INSERT INTO block VALUES ('doc_a', 'Doc A', 'text', 'root', '{}');
INSERT INTO block VALUES ('a_child_1', 'Child', 'text', 'doc_a', '{}');
INSERT INTO block VALUES ('a_child_2', 'Child', 'text', 'doc_a', '{}');
INSERT INTO block VALUES ('a_child_3', 'Child', 'text', 'doc_a', '{}');
INSERT INTO block VALUES ('doc_b', 'Doc B', 'text', 'root', '{}');
INSERT INTO block VALUES ('b_child_1', 'Child', 'text', 'doc_b', '{}');
INSERT INTO block VALUES ('b_child_2', 'Child', 'text', 'doc_b', '{}');

-- Navigate to doc_a, then doc_b
INSERT INTO navigation_history VALUES (1, 'main', 'doc_a');
INSERT OR REPLACE INTO navigation_cursor VALUES ('main', 1);
INSERT INTO navigation_history VALUES (2, 'main', 'doc_b');
INSERT OR REPLACE INTO navigation_cursor VALUES ('main', 2);

-- Verify: watch_view shows doc_b data, CDC fired for watch_view ✓
SELECT id FROM watch_view ORDER BY id;
-- → b_child_1, b_child_2, doc_b

-- Close database
```

```
Session 2 — reopen same database file:

-- Recreate upstream matviews (simulates app schema setup on restart)
DROP VIEW IF EXISTS focus_roots;
DROP VIEW IF EXISTS current_focus;

CREATE MATERIALIZED VIEW current_focus AS
  SELECT nc.region, nh.block_id
  FROM navigation_cursor nc JOIN navigation_history nh ON nc.history_id = nh.id;

CREATE MATERIALIZED VIEW focus_roots AS
  SELECT cf.region, cf.block_id, b.id AS root_id
  FROM current_focus cf JOIN block b ON b.parent_id = cf.block_id
  UNION ALL
  SELECT cf.region, cf.block_id, b.id AS root_id
  FROM current_focus cf JOIN block b ON b.id = cf.block_id;

-- NOTE: watch_view is NOT recreated — it persists from session 1

-- Navigate to doc_a
INSERT INTO navigation_history VALUES (3, 'main', 'doc_a');
INSERT OR REPLACE INTO navigation_cursor VALUES ('main', 3);

-- Check CDC events:
--   current_focus: 2 events ✓
--   focus_roots:   7 events ✓
--   watch_view:    0 events ✗  ← BUG

-- Check data:
SELECT block_id FROM current_focus WHERE region = 'main';
-- → doc_a ✓

SELECT root_id FROM focus_roots WHERE region = 'main';
-- → a_child_1, a_child_2, a_child_3, doc_a ✓

SELECT id FROM watch_view ORDER BY id;
-- → b_child_1, b_child_2, doc_b ✗  ← STALE (should be doc_a + children)
```

## Rust reproducer

File: `crates/holon/src/storage/turso_ivm_navigation_cursor_repro.rs`

```sh
cargo test -p holon turso_ivm_navigation_cursor_repro -- --nocapture
```

This test currently **fails** with the expected assertion showing the stale data.

## Root Cause Analysis

When Turso opens a database with existing matviews:
1. It reads matview definitions from `sqlite_master`
2. It reconstructs the DBSP dependency graph for all persisted matviews
3. CDC works correctly for all matviews ✓

But when some matviews are DROPped and re-CREATEd during the same session:
4. The DROP removes the matview from the DBSP graph
5. The CREATE registers a fresh DBSP node for the recreated matview
6. **Downstream matviews that were auto-loaded in step 2 are NOT reconnected** to the fresh DBSP node from step 5
7. CDC from the recreated matview never reaches the downstream auto-loaded matview

The DBSP graph edge from `focus_roots → watch_view` was established in step 2 pointing to the old `focus_roots` node. After DROP+CREATE, the new `focus_roots` node has no outgoing edge to `watch_view`.

## Affected Configuration

- Chain: `base_table → matview_A → matview_B → matview_C`
- Session startup: DROP+CREATE `matview_A` and `matview_B`, skip `matview_C`
- Result: `matview_C` is orphaned from the DBSP graph

This does NOT require recursive CTEs — the recursive CTE in the reproducer is just what the production app uses. The core issue is the DBSP graph edge not being reconnected after DROP+CREATE.

## Possible Turso Fix Directions

1. **Reconnect downstream edges after CREATE**: When a matview is created and downstream matviews already reference it (discoverable from `sqlite_master` SQL definitions), re-establish the DBSP edges.

2. **Rebuild full DBSP graph after any DDL**: After any `CREATE MATERIALIZED VIEW` or `DROP VIEW`, rebuild the entire DBSP graph from `sqlite_master`. Expensive but correct.

3. **Track dependencies bidirectionally**: When DROPping a matview, find all downstream matviews that depend on it and mark them for DBSP reconnection when the upstream is re-created.

Option 1 seems cleanest — it's a targeted fix at CREATE time.

## Workarounds (holon side)

Until fixed in Turso, holon can work around this by:
- Re-sending `CREATE MATERIALIZED VIEW IF NOT EXISTS` for all existing `watch_view_*` matviews after schema setup (forces DBSP registration)
- OR: Using `IF NOT EXISTS` for schema matviews instead of DROP+CREATE (avoids breaking the chain)

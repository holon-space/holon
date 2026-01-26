# Turso Bug Fix Request: Matview-on-Matview IVM Propagation

## Bug Description

When a materialized view (MV-B) selects from another materialized view (MV-A), changes to the base table that correctly propagate through MV-A do **not** propagate to MV-B. Both the IVM data and CDC are missing in MV-B.

## Reproduction Steps

Reproducer: `holon/crates/holon/src/storage/turso_ivm_union_all_insert_repro.rs`, test `watch_matview_on_focus_roots_cdc_after_insert`.

Schema:
```sql
CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, region TEXT NOT NULL, block_id TEXT, timestamp TEXT DEFAULT (datetime('now')));
CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER REFERENCES navigation_history(id));
CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT NOT NULL DEFAULT '', parent_id TEXT NOT NULL DEFAULT '');

-- Matview chain:
CREATE MATERIALIZED VIEW current_focus AS
  SELECT nc.region, nh.block_id, nh.timestamp
  FROM navigation_cursor nc JOIN navigation_history nh ON nc.history_id = nh.id;

CREATE MATERIALIZED VIEW focus_roots AS
  SELECT cf.region, cf.block_id, b.id AS root_id FROM current_focus AS cf JOIN block AS b ON b.parent_id = cf.block_id
  UNION ALL
  SELECT cf.region, cf.block_id, b.id AS root_id FROM current_focus AS cf JOIN block AS b ON b.id = cf.block_id;

-- The downstream "watch" matview (matview-on-matview):
CREATE MATERIALIZED VIEW mv_region_watch AS
  SELECT fr.root_id AS id, b.content, b.parent_id
  FROM focus_roots fr JOIN block b ON b.id = fr.root_id
  WHERE fr.region = 'left_sidebar';
```

Initial data:
```sql
INSERT INTO block (id, content, parent_id) VALUES ('b1', 'parent', 'doc:root');
INSERT INTO navigation_history (id, region, block_id) VALUES (1, 'left_sidebar', 'b1');
INSERT INTO navigation_cursor (region, history_id) VALUES ('left_sidebar', 1);
```

Trigger:
```sql
INSERT INTO block (id, content, parent_id) VALUES ('b2', 'child', 'b1');
```

## Expected Behavior

After inserting `b2`:
- `focus_roots` should contain `{b1, b2}` — **WORKS** (IVM + CDC both correct)
- `mv_region_watch` should contain `{b1, b2}` — **FAILS**

## Actual Behavior

- `focus_roots`: correctly updated to `{b1, b2}`, CDC fires with 1 event
- `mv_region_watch`: **still shows only `{b1}`**, 0 CDC events

The intermediate matview (`focus_roots`) updates correctly. The downstream matview (`mv_region_watch`) that `SELECT`s from `focus_roots` does not pick up the change at all — neither IVM data nor CDC.

## Key Observation

This is NOT a CDC-only issue. The **data itself** in `mv_region_watch` is stale — a direct `SELECT` against it returns `["b1"]` instead of `["b1", "b2"]`. The DBSP graph doesn't propagate changes from `focus_roots` to `mv_region_watch`.

## Relevant Code Locations (Turso)

The DBSP graph setup for materialized views. When MV-B depends on MV-A, the graph should wire MV-A's output as MV-B's input source. Likely the issue is that when MV-A is used as a source table in MV-B's definition, the DBSP pipeline doesn't subscribe MV-B to MV-A's delta stream.

Potential locations:
- Where materialized view dependencies are resolved during DDL
- Where DBSP graph nodes are connected (input wiring for matview-on-matview)
- The IVM increment propagation path

## Acceptance Criteria

- [ ] `mv_region_watch` query returns `{b1, b2}` after the insert
- [ ] CDC fires for `mv_region_watch` when its upstream matview changes
- [ ] All three tests in `turso_ivm_union_all_insert_repro.rs` pass
- [ ] Existing Turso tests still pass
- [ ] New test in Turso covers this matview-on-matview propagation case

## Run the reproducer

```bash
cd /Users/martin/Workspaces/pkm/holon
cargo test -p holon turso_ivm_union_all_insert_repro -- --nocapture
```

Test 1 (simple, no chain): PASSES
Test 2 (chained, direct focus_roots): PASSES
Test 3 (watch matview on focus_roots): FAILS ← this is the bug

# Turso IVM: Chained Matview Retains Stale Rows After Upstream UPDATE

## Summary

When a materialized view (MV-B) depends on another matview (MV-A), and MV-A's
source table is UPDATEd such that MV-A's rows change, MV-B retains stale rows
from MV-A's **previous** state. MV-A itself updates correctly.

**Severity**: High — causes the UI to display data from multiple documents mixed
together, making the application unusable for navigation.

## Production Evidence

### Schema (simplified)

```sql
-- Table: navigation_cursor (1 row per region, updated via INSERT OR REPLACE)
CREATE TABLE navigation_cursor (
    region TEXT PRIMARY KEY,
    history_id INTEGER REFERENCES navigation_history(id)
);

-- Table: navigation_history (append-only log)
CREATE TABLE navigation_history (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    region TEXT NOT NULL,
    block_id TEXT
);

-- MV-A: current_focus (JOIN between cursor and history)
CREATE MATERIALIZED VIEW current_focus AS
SELECT nc.region, nh.block_id, nh.timestamp
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- MV-B: focus_roots (depends on MV-A, joins with block table)
CREATE MATERIALIZED VIEW focus_roots AS
SELECT cf.region, cf.block_id, b.id AS root_id
FROM current_focus AS cf
JOIN block AS b ON b.parent_id = cf.block_id
UNION ALL
SELECT cf.region, cf.block_id, b.id AS root_id
FROM current_focus AS cf
JOIN block AS b ON b.id = cf.block_id;
```

### Observed State

After user navigated from document `doc:732eca1d-...` to `doc:50d389ce-...`:

**current_focus** (MV-A) — CORRECT:
```
| region | block_id                                  |
|--------|-------------------------------------------|
| main   | doc:50d389ce-b1f1-4de1-a9f1-eb19b3c245c3 |
```

**focus_roots** (MV-B) — STALE:
```
| region | block_id                                  | root_id                                      |
|--------|-------------------------------------------|----------------------------------------------|
| main   | doc:732eca1d-...  ← STALE                 | block:root-layout                            |
| main   | doc:732eca1d-...  ← STALE                 | block:0c5c95a1-5202-427f-b714-86bec42fae89   |
| main   | doc:50d389ce-...  ✓                        | block:7b960cd0-3478-412b-b96f-15822117ac14   |
| main   | doc:50d389ce-...  ✓                        | block:c74fcc72-883d-4788-911a-0632f6145e4d   |
| main   | doc:50d389ce-...  ✓                        | block:8b962d6c-0246-4119-8826-d517e2357f21   |
| main   | doc:50d389ce-...  ✓                        | block:f407a7ec-c924-4a38-96e0-7e73472e7353   |
| main   | doc:50d389ce-...  ✓                        | block:4c647dfe-0639-4064-8ab6-491d57c7e367   |
```

2 rows still reference `doc:732eca1d-...` (the **previous** navigation target).

**Raw SQL re-evaluation** of the same query — CORRECT (no stale rows):
```
| region | block_id                                  | root_id                                      |
|--------|-------------------------------------------|----------------------------------------------|
| main   | doc:50d389ce-...                          | block:92aee526-...                           |
| main   | doc:50d389ce-...                          | block:29c0aa5f-...                           |
| main   | doc:50d389ce-...                          | block:88810f15-...                           |
| main   | doc:50d389ce-...                          | block:7b960cd0-...                           |
| main   | doc:50d389ce-...                          | block:8b962d6c-...                           |
| main   | doc:50d389ce-...                          | block:c74fcc72-...                           |
| main   | doc:50d389ce-...                          | block:4c647dfe-...                           |
| main   | doc:50d389ce-...                          | block:f407a7ec-...                           |
```

8 rows, all from current document. The matview has 7 rows (5 correct + 2 stale).

### Mutation Sequence

1. App starts, `navigation_cursor.history_id = NULL` → `current_focus` empty → `focus_roots` empty
2. Startup navigation: `INSERT INTO navigation_history ... VALUES ('main', 'doc:732eca1d-...')`
3. `INSERT OR REPLACE INTO navigation_cursor ... VALUES ('main', 1)` — `current_focus` gets 1 row, `focus_roots` gets N rows for doc:732eca1d children
4. User clicks sidebar item: `INSERT INTO navigation_history ... VALUES ('main', 'doc:50d389ce-...')`
5. `INSERT OR REPLACE INTO navigation_cursor ... VALUES ('main', 2)` — `current_focus` correctly updates to doc:50d389ce
6. **BUG**: `focus_roots` still contains 2 rows from step 3 (`block_id = doc:732eca1d-...`)

### Production Context

In production, this happens during normal app startup + first navigation. The environment has:
- ~15 materialized views active simultaneously
- CDC callbacks on all views (via `set_change_callback`)
- Concurrent block insertions (org file sync running during startup)
- Recursive CTE matviews (blocks_with_paths), events matviews, navigation matviews all updating from the same INSERT statements
- ~266 blocks in the block table

### Impact

The downstream GQL query for the main panel joins on `focus_roots`:
```sql
-- Compiled from GQL
SELECT ... FROM focus_roots fr
JOIN block ON block.parent_id = fr.block_id ...
```

With stale `focus_roots` rows, the main panel receives blocks from **multiple documents** mixed together, rendering content from both the old and new navigation targets simultaneously.

## Reproducer

`examples/turso_ivm_chained_matview_stale_rows.rs` — uses the exact same schema and mutation sequence. **Does not reproduce in isolation** as of the current Turso version. The bug likely requires:

1. **More concurrent IVM cascades** — production has ~15 matviews updating from the same base tables
2. **CDC callback processing during IVM commit** — callbacks may interfere with the IVM delta computation for chained matviews
3. **Specific timing** — org file sync inserting blocks concurrently with navigation UPDATE

Run: `cargo run --example turso_ivm_chained_matview_stale_rows`

## Analysis

### Root Cause Hypothesis

When `INSERT OR REPLACE INTO navigation_cursor` fires:
1. IVM correctly computes the delta for `current_focus` (remove old row, add new row)
2. IVM should then cascade to `focus_roots`: for removed `current_focus` row, remove all `focus_roots` rows that joined with it; for added row, add new joins
3. **Bug**: The deletion cascade in step 2 doesn't fully execute — some `focus_roots` rows from the old `current_focus` row survive

This is consistent with previous IVM bugs in the JoinOperator (see `HANDOFF_TURSO_IVM_JOIN_PANIC.md`) where BTree cursor state becomes inconsistent during cascaded IVM updates.

### Relationship to Other IVM Bugs

- **HANDOFF_TURSO_IVM_JOIN_PANIC.md**: JoinOperator BTree cursor corruption during IVM cascades. Same IVM cascade path (table → MV-A → MV-B), different symptom (panic vs stale data).
- **HANDOFF_TURSO_IVM_DIRTY_PAGES.md**: Pager reads dirty pages during IVM delta computation. Could explain why the deletion delta is incomplete.
- **HANDOFF_TURSO_IVM_CHAINED_MATVIEW_RECURSION.md**: Recursive CTE over matview sources. Different but related IVM cascade path.

### Key Differentiator

Unlike the recursive CTE bugs, this bug involves a **non-recursive** chained matview (both `current_focus` and `focus_roots` are simple JOINs). The chain is:
- Table UPDATE → MV-A (simple JOIN) → MV-B (simple JOIN + UNION ALL)

The UNION ALL in MV-B may be relevant — the IVM might handle deletions correctly for one side of the UNION but not the other.

## Workaround

Drop and recreate `focus_roots` after navigation changes:

```sql
DROP VIEW IF EXISTS focus_roots;
CREATE MATERIALIZED VIEW focus_roots AS ...;
```

This forces a full recomputation. Performance impact is negligible for this small matview.

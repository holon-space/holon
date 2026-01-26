# Turso IVM Bug: Negative weight in recursive CTE materialized views

## Bug Summary

Row modifications (`UPDATE`, `INSERT OR REPLACE`, `ON CONFLICT DO UPDATE`) to a table with a recursive CTE materialized view corrupt IVM internal weights **when the matview was loaded from a persisted database** (i.e., not freshly created in the same session). The view becomes unreadable:

```
Internal error: Invalid data in materialized view: expected a positive weight, found -1
```

## Status (March 2026)

**Single-session case**: FIXED in current Turso build. The original reproducer (`turso_ivm_negative_weight_repro`) now passes — creating a matview and doing INSERT OR REPLACE in the same session works correctly.

**Cross-session case**: STILL BROKEN. When the DB is reopened and the matview already exists from a previous session, INSERT OR REPLACE corrupts the IVM weights. This is the case that hits the Holon app on every restart.

**Third-session cascade**: Even worse — after two sessions, the third session panics in Turso with `assertion failed: Mismatch in number of registers! Got 8, expected 7` at `core/incremental/expr_compiler.rs:378`.

## Reproducer

```
cargo run --example turso_ivm_negative_weight_restart_repro
```

This simulates the exact Holon app restart pattern:
1. Session 1: Create table + recursive CTE matview, insert 168 rows → OK
2. Drop connection, reopen same DB file
3. Session 2: `CREATE MATERIALIZED VIEW IF NOT EXISTS` (no-op), INSERT OR REPLACE all rows → **BUG: negative weight**
4. Session 3: Same as above → **PANIC: register mismatch**

## Minimal Trigger (cross-session)

```sql
-- Session 1
CREATE TABLE t (id TEXT PRIMARY KEY, pid TEXT, val TEXT);
CREATE MATERIALIZED VIEW mv AS
WITH RECURSIVE tree AS (
    SELECT id, pid, val, '/' || id AS path FROM t WHERE pid LIKE 'doc:%'
    UNION ALL
    SELECT c.id, c.pid, c.val, p.path || '/' || c.id
    FROM t c INNER JOIN tree p ON c.pid = p.id
) SELECT * FROM tree;
INSERT INTO t VALUES ('r0', 'doc:d1', 'H0');
-- ... (8 roots × 21 depth = 168 rows)
SELECT COUNT(*) FROM mv;  -- 168 ✓

-- Close connection, reopen same DB file

-- Session 2
CREATE MATERIALIZED VIEW IF NOT EXISTS mv AS ...;  -- no-op
INSERT OR REPLACE INTO t VALUES ('r0', 'doc:d1', 'H0 v2');
-- ... (all 168 rows)
SELECT COUNT(*) FROM mv;
-- ERROR: expected a positive weight, found -1
```

## What DOES matter

1. **Recursive CTE** in the materialized view definition
2. **Row modification** after initial INSERT
3. **Cross-session**: the matview must have been loaded from disk (persisted from a previous session)
4. **Scale**: ~168 rows triggers it reliably in cross-session. In single-session with other matviews, fewer rows suffice

## What does NOT matter

- CDC callbacks
- Other matviews on the same table
- Chained matviews
- `OR` in the base case WHERE clause
- `INSERT OR REPLACE` vs `UPDATE` vs `ON CONFLICT DO UPDATE` (all trigger it)

## Impact in Holon

Every app restart triggers this because:
1. Org file sync does `INSERT OR REPLACE` for each block (SqlOperationProvider.create)
2. `block_with_path` is a recursive CTE matview on the `block` table
3. After sync, `render_block()` calls `lookup_block_path()` which queries `block_with_path`

## Root Cause (hypothesis)

Turso's DBSP incremental maintenance for recursive CTEs doesn't correctly restore its internal circuit state from disk. When the matview is freshly created (same session), the DBSP state is in memory and deltas are computed correctly. But when the DB is reopened and the DBSP state is deserialized from the `__turso_internal_dbsp_state_v1_*` tables, something goes wrong — the delta computation for UPDATE produces incorrect weights.

The third-session panic (`register mismatch`) suggests the corruption is cumulative and eventually causes the expr compiler to produce inconsistent register counts.

## Where to Look in Turso

- `core/incremental/` — DBSP circuit construction and **state serialization/deserialization**
- `core/incremental/expr_compiler.rs:378` — the register mismatch panic on third session
- The `__turso_internal_dbsp_state_v1_*` tables — how weights are persisted and restored
- Specifically: how the recursive CTE fixpoint state is serialized vs how it's recreated on DB reopen
- Compare behavior of freshly-created matview vs deserialized matview for the same UPDATE operation

# Turso IVM Bug: "dirty pages should be empty for read txn"

## Bug Summary

Two related bugs in Turso's IVM (Incremental View Maintenance):

1. **Primary**: `pager.rs:4699` — assertion `dirty pages should be empty for read txn` fires during IVM processing
2. **Secondary**: `incremental/persistence.rs:152` — `Index points to non-existent table row` after the primary bug corrupts the BTree index

## Reproduction Context

Observed in holon's `general_e2e_pbt_sql_only` property-based test. The test uses a single `TursoBackend` actor that serializes all SQL through one connection. This is **not** a cross-connection race.

### Schema when bug triggers

```sql
-- Base table
CREATE TABLE blocks (id TEXT PK, parent_id TEXT, content TEXT, ...);

-- Recursive CTE matview (heavy IVM work per insert)
CREATE MATERIALIZED VIEW blocks_with_paths AS
WITH RECURSIVE paths AS (
  SELECT id, parent_id, ..., '/' || id as path FROM blocks
  WHERE parent_id LIKE 'doc:%' OR parent_id LIKE 'sentinel:%'
  UNION ALL
  SELECT b.id, b.parent_id, ..., p.path || '/' || b.id FROM blocks b
  INNER JOIN paths p ON b.parent_id = p.id
) SELECT * FROM paths;

-- JOIN matview (smaller but still IVM-active)
CREATE MATERIALIZED VIEW current_focus AS
SELECT nc.region, nh.block_id FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

-- Simple filtered matview (event bus)
CREATE MATERIALIZED VIEW events_view_block AS
SELECT * FROM events WHERE status = 'confirmed' AND aggregate_type = 'block';

-- Several watch_view_XXXX matviews (SELECT ... FROM blocks WHERE ...)
```

A CDC callback is registered via `set_change_callback()`.

### Trigger sequence (single connection, serialized)

1. `BulkExternalAdd` transition inserts 5-20 blocks rapidly (one INSERT per block)
2. Each INSERT triggers IVM update on `blocks_with_paths` (recursive CTE) + all `watch_view_*` matviews
3. Concurrently (via tokio tasks, but serialized through actor), `query_and_watch` calls create new matviews (`CREATE MATERIALIZED VIEW IF NOT EXISTS watch_view_XXX AS ...`)
4. During IVM processing of one of these operations, the pager panics

### What happens after the panic

The actor catches the panic via `catch_unwind()` and continues processing commands. But the database is now in a corrupted state:

```
[TursoBackend::Actor] Caught panic during command processing: dirty pages should be empty for read txn. Actor continues.
```

All subsequent queries to matview-backed tables fail:
```
SQL execution failed: Query error: Failed to fetch row: Internal error: Index points to non-existent table row
```

This repeats indefinitely — the BTree index is permanently inconsistent until the database is deleted and recreated.

## Root Cause Hypothesis

Inside `pager.rs:rollback()` (line 4684):

```rust
pub fn rollback(&self, schema_did_change: bool, connection: &Connection, is_write: bool) {
    if is_write {
        self.clear_page_cache(clear_dirty);
        self.dirty_pages.write().clear();
    } else {
        turso_assert!(
            self.dirty_pages.read().is_empty(),    // <-- PANICS HERE
            "dirty pages should be empty for read txn"
        );
    }
```

During IVM processing of a write (INSERT into `blocks`), the engine:
1. Opens a write transaction (marks pages dirty)
2. IVM computes deltas for each matview, which involves **read cursors** to scan existing matview data
3. Some internal codepath calls `rollback()` with `is_write=false` (for a read sub-operation)
4. But dirty pages exist from the parent write transaction
5. Assertion fires

The `dirty_pages` set is shared across the connection — there's no isolation between the parent write transaction and IVM's internal read operations.

## Why the standalone reproducer doesn't trigger it

The turso Rust bindings serialize operations through `conn.execute()` which runs each statement to completion before returning. The bug requires the IVM processing to hit a specific codepath where:
- Multiple matviews (especially recursive CTE + JOIN) are updated in cascade
- The internal delta computation opens a read cursor that crosses a page boundary
- The pager's `rollback()` is called for this read cursor before the write transaction's dirty pages are cleared

This is highly dependent on the size of the BTree (number of blocks) and the depth of the recursive CTE hierarchy. The holon PBT generates random block structures that occasionally hit the right page-boundary conditions.

## Reproducer

The most reliable reproducer is:
```bash
cd /path/to/holon
cargo test -p holon-integration-tests --test general_e2e_pbt general_e2e_pbt_sql_only -- --nocapture 2>&1 | tee /tmp/pbt.txt
```

Run it a few times — the bug appears in ~50% of runs.

A standalone example is in `crates/holon/examples/turso_ivm_dirty_pages_repro.rs` but it hasn't triggered the bug yet because it doesn't generate enough BTree pressure to hit the right page-boundary conditions.

## Suggested investigation approach

1. In `pager.rs:rollback()`, **don't assert** — instead log + skip the assertion, and check if this causes data corruption or if the dirty pages are harmless (leftover from the parent write txn that will be committed later)

2. Add tracing to `incremental/persistence.rs` around line 150 to log the rowid that the index points to vs. what the table cursor finds — this will confirm whether the index corruption is caused by the pager panic (partial rollback) or is a separate issue

3. Check if IVM's read cursors should be using a separate pager or snapshot to avoid seeing the parent transaction's dirty pages

## Files

- `pager.rs:4699` — the assertion that panics
- `incremental/persistence.rs:152` — the index-vs-table mismatch error
- `holon/src/storage/turso.rs:1297` — actor's `catch_unwind()` that absorbs the panic
- `holon-integration-tests/src/pbt/sut.rs:483` — PBT code where the error propagates

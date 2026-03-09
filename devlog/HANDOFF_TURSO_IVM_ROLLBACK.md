# Turso IVM Bug: ROLLBACK does not undo materialized view state

## Summary

When a transaction containing DML (INSERT/UPDATE/DELETE) is ROLLBACKed, the base table changes are correctly undone via WAL rollback, but the IVM/DBSP circuit state and the matview btree are NOT rolled back. The matview retains stale deltas from the rolled-back transaction.

## Impact

Any app that uses BEGIN/ROLLBACK with materialized views will see incorrect matview data after rollback. The matview shows rows that should have been removed (or missing rows that should have been restored).

## Reproducer (pure SQL)

```sql
CREATE TABLE t(a INTEGER, b INTEGER);
INSERT INTO t VALUES (1, 10), (2, 20), (3, 30);
CREATE MATERIALIZED VIEW v AS SELECT * FROM t WHERE b > 15;

SELECT * FROM v ORDER BY a;
-- → 2|20, 3|30 ✓

BEGIN;
INSERT INTO t VALUES (4, 40), (5, 50);
SELECT * FROM v ORDER BY a;
-- → 2|20, 3|30, 4|40, 5|50 ✓ (uncommitted rows visible)

ROLLBACK;
SELECT * FROM v ORDER BY a;
-- → 2|20, 3|30, 4|40, 5|50 ✗ BUG (should revert to 2|20, 3|30)
```

Same pattern reproduces with DELETE, UPDATE, aggregations (COUNT DISTINCT), JOINs, and filtered views. 9 existing sqltest cases capture this:

- `matview-rollback-insert`
- `matview-rollback-delete`
- `matview-rollback-update`
- `matview-rollback-aggregation`
- `matview-rollback-mixed-operations`
- `matview-rollback-filtered-aggregation`
- `matview-rollback-empty-view`
- `matview-count-distinct-transactions` (INSERT + ROLLBACK portion)
- `matview-join-rollback`

All in `testing/sqltests/turso-tests/materialized_views.sqltest`.

## Root Cause Analysis

The commit path works: `commit_txn()` → `apply_view_deltas()` → `merge_delta()` writes deltas to the matview btree. But rollback only calls `pager.rollback()` which reverts page-level changes to the base tables.

The problem is that `apply_view_deltas()` runs eagerly — it writes to the matview btree during the transaction (before COMMIT), so the matview btree pages have already been modified. When `pager.rollback()` runs, it reverts the WAL, but the matview btree mutations were part of the same WAL and should be reverted too.

**Hypothesis 1 (most likely)**: The matview btree writes happen through the same pager and SHOULD be reverted by `pager.rollback()`. But the in-memory DBSP operator state (seen_counts, seen_rows, union_all_rowids, next_rowid, JoinOperator stored state) is NOT reverted. On the next transaction, the DBSP circuit produces wrong deltas because its in-memory state reflects the rolled-back transaction.

**Hypothesis 2**: The matview btree writes happen outside the WAL transaction boundary (e.g., in auto-commit mode on a separate internal path), so `pager.rollback()` doesn't touch them.

**Hypothesis 3**: The `view_transaction_states` HashMap that accumulates deltas during a transaction is not cleared on rollback, so stale deltas leak into the next commit.

Validate hypothesis 1 first — it's the most likely given that the pager should handle btree rollback. Add logging or breakpoints in `rollback()` and `apply_view_deltas()` to trace the order of operations.

## Key Files

| File | What to look at |
|------|----------------|
| `core/vdbe/mod.rs` | `apply_view_deltas()` — when/how matview btree writes happen |
| `core/vdbe/mod.rs` | `rollback_txn()` or equivalent — what cleanup happens on ROLLBACK |
| `core/incremental/compiler.rs` | `DbspCircuit` state — `seen_counts`, `seen_rows`, operator state that persists across transactions |
| `core/incremental/view.rs` | `merge_delta()` — writes to matview btree |
| `core/incremental/recursive_operator.rs` | `union_all_rowids`, `next_rowid` — mutable state that survives rollback |
| `core/storage/pager.rs` | `rollback()` — does it revert matview btree pages? |

## Possible Fix Directions

1. **Snapshot and restore DBSP operator state on rollback**: Before applying deltas, snapshot the relevant DBSP state. On rollback, restore the snapshot. This is the surgical fix but requires identifying ALL mutable operator state.

2. **Defer matview btree writes until COMMIT**: Don't call `apply_view_deltas()` / `merge_delta()` until `commit_txn()`. During the transaction, matview reads use the in-memory delta overlay (which `MaterializedViewCursor::ensure_tx_changes_computed()` already computes). On ROLLBACK, just discard the deltas. This is cleaner but may require reworking the read path.

3. **Reset DBSP circuit state on rollback**: After `pager.rollback()`, call a new method on each `IncrementalView` / `DbspCircuit` that resets operator state to match the btree (similar to `restore_recursive_operators_if_needed()` which already exists for the reopen case). This is the pragmatic fix — reuse existing restore logic.

Option 3 seems most practical given that `restore_recursive_operators_if_needed()` already handles a similar "state out of sync with btree" scenario.

## Negotiable: Attached Database CDC (3 tests)

These 3 tests fail with `Parse error: no such table: t1`:
- `attach-write-cdc-insert`
- `attach-write-cdc-delete`
- `attach-write-cdc-update`

The CDC path uses unqualified table names to look up the table in the main schema, but the table lives in an attached database (`aux.t1`). Fixing this requires the CDC machinery to resolve table names across database boundaries.

**Skip these unless you have a use-case for CDC on attached databases.** The fix likely involves threading the database ID through the CDC callback path and looking up the table in the correct schema. This is plumbing work with no current user demand.

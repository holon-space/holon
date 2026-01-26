# Turso IVM: Pager Crash During Chained Matview CASCADE

## Summary

When a materialized view chain is 3+ levels deep (`table → MV-A → MV-B → MV-C`)
and the base table is mutated with `INSERT OR REPLACE`, the IVM cascade crashes
with a subtraction overflow in `Pager::allocate_page`. The crash occurs during
`JoinOperator::commit` when writing IVM delta rows to the BTree.

**Severity**: Critical — Turso process crashes (panic). Deterministic with
sufficient data volume (~100 block rows). When it doesn't crash, the downstream
matview retains stale rows (see HANDOFF_TURSO_IVM_CHAINED_MATVIEW_STALE_ROWS.md).

**Reproduction rate**: 100% with the provided SQL replay (218 statements).

## Stack Trace

```
thread 'main' panicked at turso/core/storage/pager.rs:4621:45:
attempt to subtract with overflow

 3: turso_core::storage::pager::Pager::allocate_page
 4: turso_core::storage::pager::Pager::do_allocate_page
 5: turso_core::storage::btree::BTreeCursor::balance_non_root
 6: turso_core::storage::btree::BTreeCursor::balance
 7: turso_core::storage::btree::BTreeCursor::insert_into_page
 8: <BTreeCursor as CursorTrait>::insert
 9: turso_core::incremental::persistence::WriteRow::write_row
10: <JoinOperator as IncrementalOperator>::commit
11: turso_core::incremental::compiler::DbspNode::process_node
12: turso_core::incremental::compiler::DbspCircuit::execute_node
```

## Matview Chain

```
navigation_cursor (table, INSERT OR REPLACE)
  → current_focus (MV-A: simple JOIN with navigation_history)
    → focus_roots (MV-B: JOIN + UNION ALL with block table)
      → watch_view_eb3125ab79aead8f (MV-C: recursive CTE over focus_roots + block)
```

The crash occurs at MV-C's `JoinOperator::commit`. MV-A and MV-B are simple
JOINs; MV-C uses a recursive CTE that walks the block tree from `focus_roots`
root_ids.

## Triggering Statement

```sql
INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 5);
```

This is the 5th navigation in a sequence. The crash requires:
1. At least ~100 rows in the `block` table (threshold is sharp: 75 blocks = no crash, 100 = 100% crash)
2. The 3-level matview chain to be active
3. Multiple navigation cycles (crash occurs around the 5th `INSERT OR REPLACE`)

## Reproducer

### SQL Replay (recommended)

The minimal reproducer is a 218-statement SQL file that can be replayed with
the Rust replayer:

```bash
cargo run --example turso_sql_replay -- \
  crates/holon/examples/turso_ivm_pager_crash_replay.sql
```

The replayer:
- Creates a fresh Turso database with `experimental_materialized_views(true)`
- Executes all statements sequentially (no concurrency, no CDC callbacks)
- Crashes deterministically at the 5th navigation INSERT OR REPLACE

### Manual

```bash
# The SQL file can also be piped through tursodb directly, but the crash
# manifests differently (tursodb may catch the panic internally):
cat crates/holon/examples/turso_ivm_pager_crash_replay.sql | \
  /path/to/tursodb --experimental-views /tmp/crash-test.db
```

## Key Observations

1. **No concurrency required**: Single connection, sequential execution
2. **No CDC callbacks required**: Crash occurs without `set_change_callback`
3. **Data volume matters**: Needs ~100+ rows in the `block` table that the
   matviews join against. Below ~75 rows, no crash.
4. **Non-deterministic variant**: When the crash doesn't occur (rare with
   sufficient data), `focus_roots` matview has 0 rows while raw SQL
   re-evaluation returns 10-22 rows — the stale rows bug from
   HANDOFF_TURSO_IVM_CHAINED_MATVIEW_STALE_ROWS.md.
5. **Register mismatch variant**: Sometimes manifests as
   `expr_compiler.rs:378: assertion failed: Mismatch in number of registers!
   Got 46, expected 39` instead of the pager crash.

## Relationship to Other IVM Bugs

- **HANDOFF_TURSO_IVM_CHAINED_MATVIEW_STALE_ROWS.md**: Same root cause. When
  the pager crash doesn't occur, the IVM silently produces incorrect results
  instead (stale rows, missing rows).
- **HANDOFF_TURSO_IVM_DIRTY_PAGES.md**: Related — pager reads dirty pages
  during IVM delta computation. The subtraction overflow in `allocate_page`
  may be caused by the pager's free page count being corrupted by dirty reads.
- **HANDOFF_TURSO_IVM_JOIN_PANIC.md**: Same JoinOperator code path. BTree
  cursor state corruption during cascaded IVM updates.

## Files

- `crates/holon/examples/turso_sql_replay.rs` — Rust replayer with CDC support
- `crates/holon/examples/turso_ivm_pager_crash_replay.sql` — Minimal SQL (218 stmts)
- `scripts/extract-sql-trace.py` — Extract annotated SQL from `HOLON_TRACE_SQL=1` logs
- `docs/HANDOFF_TURSO_IVM_CHAINED_MATVIEW_STALE_ROWS.md` — Related stale rows bug

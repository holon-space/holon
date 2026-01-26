# Handoff: Turso IVM JoinOperator BTree Cursor Corruption

## Problem

BTree cursor corruption in Turso's IVM (Incremental View Maintenance) when **chained materialized views** (a matview that JOINs with another matview) exist alongside other matviews with CDC callbacks active.

The panic manifests as `PageStack::current` with `current_page=-1` during `JoinOperator::commit`, which permanently corrupts the JoinOperator into an `Invalid` state. Every subsequent IVM update to that operator panics again.

## Reproducer

**File**: `turso/tests/integration/query_processing/test_ivm_join_cursor_corruption.rs`

**Run**:
```bash
cargo test --test integration_tests test_ivm_join_cursor_corruption_chained_watch_views -- --nocapture
```

**Result**: Panics consistently with:
```
[PageStack::current] current_page=-1 is negative! stack_depth=0, loaded_pages=[].
This indicates the cursor was used after clear() without push().
```

## Root Cause

The bug requires **chained matview dependencies** — specifically a matview that JOINs with another matview. Without this chaining, even hundreds of inserts across multiple matviews work fine.

### Required schema (3 levels of matview dependencies):

```
Level 0 (base tables): blocks, events, navigation_cursor, navigation_history
Level 1 (base matviews): blocks_with_paths (recursive CTE), events_view_block (filter), current_focus (JOIN)
Level 2 (chained): watch_view_main JOINs with current_focus, watch_view_sidebar JOINs with current_focus
```

### What does NOT trigger the bug (verified by tests that pass):
- Multiple level-1 matviews without chaining (recursive CTE + filter + JOIN) — even with 200+ inserts
- Interleaved inserts across blocks + events + navigation changes
- Deeply nested hierarchies stressing the recursive CTE
- Any combination above without level-2 chained matviews

### What DOES trigger the bug:
- Level-2 matviews that JOIN with `current_focus` (itself a matview) — specifically `watch_view_main` and `watch_view_sidebar`
- Block inserts that trigger IVM cascades through all 3 levels
- CDC callbacks active during the cascade

### Crash path:

```
conn.execute("INSERT INTO blocks ...")
  → commit_txn
    → apply_view_deltas (BFS collects all transitively dependent views)
      → IncrementalView::merge_delta
        → DbspCircuit::commit → run_circuit → execute_node (4 levels deep)
          → JoinOperator::commit
            → WriteRow::write_row
              → BTreeCursor::insert → insert_into_page
                → PageStack::top → PANIC (current_page=-1)
```

The `execute_node` recurses 4 levels deep because the DBSP circuit graph for the chained views has nested dependencies. During this deep traversal, the BTree cursor's page stack is cleared but never re-pushed before use.

### Secondary damage (the `Invalid` state loop):

In `join_operator.rs`, `commit()` does `mem::replace(&mut self.commit_state, JoinCommitState::Invalid)` as a sentinel before processing. If the processing panics (as above), the state is left as `Invalid`. The `return_and_restore_if_io!` macro only restores on IO or Error, not on panic. After the first panic, every subsequent IVM update hits the `Invalid` match arm and panics again — creating an infinite panic loop in production (136 panics in one startup).

## Affected Code

- **Panic site**: `turso/core/storage/btree.rs:6684` — `PageStack::current` with negative index
- **Cursor user**: `turso/core/incremental/persistence.rs:298` — `WriteRow::write_row`
- **State corruption**: `turso/core/incremental/join_operator.rs:646` — `mem::replace` sentinel
- **Invalid loop**: `turso/core/incremental/join_operator.rs:769` — `JoinCommitState::Invalid` panic
- **IVM cascade**: `turso/core/incremental/compiler.rs:872,891,993` — `execute_node` recursion
- **Holon actor**: `holon/crates/holon/src/storage/turso.rs:1297` — catches panics but can't recover operator state

## Possible Fixes

### Fix 1: Fix cursor lifecycle in cascading execute_node
The real bug. During deeply nested `execute_node` calls for chained matviews, a BTree cursor from an earlier node's commit is cleared/recycled but then accessed by a later node's `WriteRow::write_row`. The cursor needs to either:
- Be re-acquired per node (not shared across cascade levels)
- Have its page stack properly restored after the parent node's commit completes

### Fix 2: Reset JoinOperator on panic recovery
Mitigates the infinite panic loop. After catching a panic in the actor, reset the JoinOperator's `commit_state` back to `Idle`. This doesn't fix the root cause but prevents one BTree corruption from taking down all subsequent IVM updates.

### Fix 3: Use `Option<JoinCommitState>` instead of `Invalid` sentinel
Replace `mem::replace` with `Option::take()`. This makes the "no valid state" case explicit and allows recovery without a separate `Invalid` variant that panics.

## Impact

Blocks any app using chained materialized views (matview-on-matview JOINs) with CDC from doing bulk inserts. In Holon, this blocks fresh-DB startup when org sync inserts blocks while watch views are active.

## Discovery Context

Found in the Holon PKMS Flutter app during startup. The app creates:
- `blocks_with_paths` (recursive CTE on blocks table) — via schema module
- `events_view_block` (filter on events table) — via TursoEventBus::subscribe()
- `current_focus` (JOIN on navigation tables) — via schema module
- `watch_view_*` (JOINs with current_focus) — dynamically via BackendEngine::query_and_watch()

The original log also showed a `ParentRef` deserialization error just before the first panic, but this is unrelated — it's a separate holon-side bug where a raw UUID string is stored as `parent_id` instead of a `ParentRef` enum variant.

## Log File

Production crash log: `/tmp/flutter.log` (run Flutter app with holon-pkm orgmode root)

# Turso IVM Bug Report: Literal values in JOIN conditions cause parse error and subsequent panics

## Summary

Turso's incremental materialized views (IVM) do not support literal values in JOIN conditions. While this limitation produces a clear error message, subsequent operations on the database can trigger panics, suggesting state corruption.

## Environment

- Turso libSQL (local fork at `/Users/martin/Workspaces/bigdata/turso`)
- Using incremental materialized views with CDC (Change Data Capture)

## Bug 1: Parse Error for Literal in JOIN Condition

### Minimal Reproducer

```sql
CREATE TABLE items (id TEXT PRIMARY KEY, parent_id TEXT);
CREATE TABLE cursors (region TEXT PRIMARY KEY, item_id TEXT);

-- This FAILS:
CREATE MATERIALIZED VIEW broken_view AS
SELECT i.id
FROM items i
LEFT JOIN cursors c ON c.region = 'main'
WHERE i.parent_id = c.item_id;
```

### Error Message

```
Parse error: Only simple column references are supported in join conditions for incremental views
```

### Expected Behavior

Either:
1. Support literal values in join conditions for IVM, OR
2. Document this limitation clearly

### Workaround

Move the literal filter from the JOIN condition to the WHERE clause, and use INNER JOIN (LEFT JOIN not supported):

```sql
-- Instead of: LEFT JOIN cursors c ON c.region = 'main'
-- Use:
CREATE MATERIALIZED VIEW working_view AS
SELECT i.id
FROM items i
JOIN cursors c ON i.parent_id = c.item_id
WHERE c.region = 'main';
```

**Turso IVM limitations:**
- No literal values in JOIN conditions
- No subqueries in JOIN clauses
- No LEFT/RIGHT OUTER JOINs

## Bug 2: JOIN Materialized Views Panic on Data Insert

Even when a JOIN materialized view is created successfully, inserting data into the base tables causes a panic in the JoinOperator during commit.

### Stack Trace

```
turso_core::storage::btree::PageStack::top
turso_core::storage::btree::BTreeCursor::insert_into_page
<turso_core::storage::btree::BTreeCursor as turso_core::storage::btree::CursorTrait>::insert
turso_core::incremental::persistence::WriteRow::write_row
<turso_core::incremental::join_operator::JoinOperator as turso_core::incremental::operator::IncrementalOperator>::commit
```

### Reproducer

```sql
-- Create tables
CREATE TABLE a (id INTEGER PRIMARY KEY, val TEXT);
CREATE TABLE b (id INTEGER PRIMARY KEY, a_id INTEGER);

-- Create JOIN materialized view (succeeds)
CREATE MATERIALIZED VIEW ab_view AS
SELECT a.id, a.val, b.id as b_id
FROM a JOIN b ON b.a_id = a.id;

-- Insert data (PANICS!)
INSERT INTO a VALUES (1, 'test');
```

### Workaround

Use regular `CREATE VIEW` instead of `CREATE MATERIALIZED VIEW` for views with JOINs.
The view won't have IVM/CDC capabilities, but it won't panic.

## Bug 3: Nested Materialized View JOINs - BTree Cursor Not Initialized

**Root Cause Identified**: When a materialized view JOINs with another materialized view that itself has a JOIN, the IVM system fails to properly initialize BTree cursors during cascading updates.

### Panic Message (with debug logging)

```
[PageStack::current] current_page=-1 is negative! stack_depth=0, loaded_pages=[].
This indicates the cursor was used after clear() without push().
```

### Scenario That Triggers the Bug

1. Create base tables: `navigation_history`, `navigation_cursor`, `blocks`
2. Create first matview with JOIN:
   ```sql
   CREATE MATERIALIZED VIEW current_focus AS
   SELECT nc.region, nh.block_id, nh.timestamp
   FROM navigation_cursor nc
   JOIN navigation_history nh ON nc.history_id = nh.id;
   ```
3. Create second matview that JOINs with the first:
   ```sql
   CREATE MATERIALIZED VIEW watch_view AS
   SELECT blocks.id, blocks.parent_id, blocks.content
   FROM blocks
   INNER JOIN current_focus AS cf ON blocks.parent_id = cf.block_id
   WHERE cf.region = 'main';
   ```
4. Execute any operation that modifies `navigation_history` or `navigation_cursor`
   - This triggers IVM update on `current_focus`
   - Which cascades to `watch_view`
   - **PANIC** occurs during the cascading update

### What We Know

- The `PageStack` is completely empty: `stack_depth=0, loaded_pages=[]`
- `clear()` was called on the cursor but `push()` was never called
- The cursor is being used without being initialized with a root page
- This only occurs with **nested** matview JOINs (matview A joins matview B, where B has a JOIN)
- Does NOT reproduce in isolated unit tests - only with CDC enabled in full application

### Steps to Reproduce

```sql
-- Setup base tables
CREATE TABLE blocks (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT);
CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, region TEXT, block_id TEXT);
CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER);

INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL);

-- Create nested matview structure
CREATE MATERIALIZED VIEW current_focus AS
SELECT nc.region, nh.block_id
FROM navigation_cursor nc
JOIN navigation_history nh ON nc.history_id = nh.id;

CREATE MATERIALIZED VIEW watch_view AS
SELECT blocks.id, blocks.content
FROM blocks
INNER JOIN current_focus cf ON blocks.parent_id = cf.block_id
WHERE cf.region = 'main';

-- Trigger the bug: insert into navigation_history
INSERT INTO navigation_history (region, block_id) VALUES ('main', 'some-block');
UPDATE navigation_cursor SET history_id = 1 WHERE region = 'main';
-- ^ This triggers cascading IVM update -> PANIC
```

### Full Stack Trace

```
   2: turso_core::storage::btree::PageStack::top
   3: turso_core::storage::btree::BTreeCursor::insert_into_page
   4: <turso_core::storage::btree::BTreeCursor as turso_core::storage::btree::CursorTrait>::insert
   5: turso_core::incremental::persistence::WriteRow::write_row
   6: <turso_core::incremental::join_operator::JoinOperator as turso_core::incremental::operator::IncrementalOperator>::commit
   7: turso_core::incremental::compiler::DbspNode::process_node
   8: turso_core::incremental::compiler::DbspCircuit::execute_node
   9: turso_core::incremental::compiler::DbspCircuit::execute_node  <-- recursive!
  10: turso_core::incremental::compiler::DbspCircuit::run_circuit
  11: turso_core::incremental::compiler::DbspCircuit::commit
  12: turso_core::incremental::view::IncrementalView::merge_delta
  13: turso_core::vdbe::Program::apply_view_deltas
  14: turso_core::vdbe::Program::commit_txn
  15: turso_core::vdbe::execute::halt
```

### Root Cause Analysis (from trace logs)

The trace logs reveal what's happening:

1. First `JoinOperator::commit` starts: `left_changes=0, right_changes=1`
2. State transitions: `Idle -> Eval`
3. During Eval, VDBE executes `halt(auto_commit=true)`
4. `halt` triggers `commit_txn` → `apply_view_deltas`
5. **SECOND** `JoinOperator::commit` is called (for nested view) while first is still in Eval!
6. Second commit sees state `Eval` instead of `Idle` (state corruption)
7. Cursors from `DbspStateCursors` are shared/reused between the two commits
8. The cursor for the nested view's DBSP state index gets cleared by one operation but not repopulated before the next operation uses it

**Key log sequence showing re-entrancy:**
```
387030Z: [JoinOperator::commit] Starting commit, left_changes=0, right_changes=1
387043Z: [JoinOperator::commit] Current state: Idle
387055Z: [JoinOperator::commit] Idle -> Eval
387520Z: halt(auto_commit=true)  <-- triggers nested commit!
387545Z: [JoinOperator::commit] Starting commit, left_changes=0, right_changes=1
387558Z: [JoinOperator::commit] Current state: Eval  <-- SHOULD BE IDLE!
```

### Suggested Fix

The `JoinOperator::commit` function is being called re-entrantly during nested matview updates. Either:
1. Prevent re-entrancy by completing the outer commit before starting inner commits
2. Use separate cursor instances for each level of nesting
3. Add state machine guards to detect and handle re-entrant calls

## SQL Reproducer Script

See: `turso-join-literal-bug.sql` in the same directory.

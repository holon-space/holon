# Turso Bug Fix: pager.rs underflow panic in allocate_page during IVM

## Bug Description

`pager.rs:4632` and `pager.rs:4556` panic with "attempt to subtract with overflow" during B-tree page allocation. The panic occurs in `allocate_page` → called from `balance_quick` → `insert_into_page` → `insert` during IVM processing of large transactions.

The panic corrupts the connection state (leaves a transaction open), which cascades into "cannot start a transaction within a transaction" errors on subsequent operations, effectively killing all CDC and matview updates.

### Transaction fixes already applied (holon side)
Three fixes were applied to `holon/crates/holon/src/storage/turso.rs`:
1. ROLLBACK on COMMIT failure
2. Self-healing BEGIN (rollback stale transaction + retry)
3. Transaction cleanup after panic (catch_unwind handler)

These prevent the cascading failure but the pager panic is the root cause.

## Evidence from production

Stack trace from `/tmp/holon-gpui.log`:

```
thread 'tokio-rt-worker' panicked at core/storage/pager.rs:4632:45:
attempt to subtract with overflow

   3: allocate_page        at pager.rs:4632:45
   4: do_allocate_page     at pager.rs:2405:39
   5: balance_quick        at btree.rs:2564:59
   6: balance              at btree.rs:2538:40
   7: insert_into_page     at btree.rs:2402:40
   8: insert               at btree.rs:5169:28
   9: op_insert            at vdbe/execute.rs:8608:42
  10: normal_step          at vdbe/mod.rs:1339:19
```

Also panics at `pager.rs:4556:45` with the same error in a separate invocation.

### Trigger context
- A 256-statement transaction inserting directory events into the `events` table
- The `events` table has a materialized view (`events_view_directory`) with IVM active
- IVM processing of the bulk insert triggers B-tree balancing which hits the underflow
- The panic happens repeatedly (4 times in one session) on different transactions

## Reproduction

### Suggested reproduction approach

```rust
let conn = db.connect()?;

// 1. Create events table with matview (IVM active)
conn.execute("CREATE TABLE events (
    id TEXT PRIMARY KEY,
    event_type TEXT NOT NULL,
    aggregate_type TEXT NOT NULL,
    aggregate_id TEXT,
    payload TEXT DEFAULT '{}'
)", ()).await?;

conn.execute("CREATE MATERIALIZED VIEW events_view AS
    SELECT * FROM events WHERE aggregate_type = 'directory'
", ()).await?;

// 2. Bulk insert in a transaction (256+ rows)
conn.execute("BEGIN TRANSACTION", ()).await?;
for i in 0..256 {
    conn.execute(&format!(
        "INSERT INTO events (id, event_type, aggregate_type, aggregate_id, payload) \
         VALUES ('evt-{i}', 'directory.created', 'directory', 'dir-{i}', \
         '{{\"change_type\":\"created\",\"data\":{{\"id\":\"dir-{i}\"}}}}')"
    ), ()).await?;
}
conn.execute("COMMIT", ()).await?;
// Expected: no panic
// Actual: pager.rs:4632 panics with "attempt to subtract with overflow"
```

Key ingredients:
- Materialized view with IVM active on the table being bulk-inserted into
- Large transaction (256+ INSERT statements)
- B-tree balancing triggers page allocation during IVM processing

## Analysis

### Relevant Turso code locations
- `core/storage/pager.rs:4632` and `pager.rs:4556` — `allocate_page` underflow
- `core/storage/pager.rs:2405` — `do_allocate_page` caller
- `core/storage/btree.rs:2564` — `balance_quick` triggers allocation
- IVM DBSP state tables (`__turso_internal_dbsp_state_v1_*`) store intermediate results

### Root cause hypothesis

During IVM processing of a large transaction, the DBSP state B-tree grows and needs page allocation via `balance_quick`. The `allocate_page` function subtracts a value that underflows (goes below zero), likely because:
1. Free page count tracking gets out of sync during large IVM cascades
2. The DBSP state tables accumulate many pages during bulk inserts, exhausting the free list
3. A counter wraps or a boundary condition is missed in the page allocator

## Acceptance Criteria
- [ ] No panic during bulk INSERT into table with active matview
- [ ] Existing Turso tests pass
- [ ] New test: 256+ row transaction with IVM matview → no panic
- [ ] Holon's matview CDC pipeline works end-to-end after fix

## Turso Repo
`~/Workspaces/bigdata/turso/` (branch: `holon`)

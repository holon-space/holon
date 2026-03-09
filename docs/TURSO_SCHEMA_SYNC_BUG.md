# Turso Bug Report: Schema Not Synchronized Between Connections

## Summary

Materialized view schema changes made on one connection are not visible to other existing connections. This causes IVM (Incremental View Maintenance) to silently fail when inserts are made on a different connection than the one that created the view.

## Environment

- Turso version: 0.4.0 (local fork)
- Rust version: stable

## Steps to Reproduce

1. Open database and create connection A
2. Create connection B (write connection with CDC callback)
3. On connection A: `CREATE MATERIALIZED VIEW myview AS SELECT * FROM mytable WHERE status = 'active'`
4. On connection B: `INSERT INTO mytable (status) VALUES ('active')`

**Expected**: IVM triggers and CDC callback fires for the new row matching the view
**Actual**: No IVM trigger, no CDC callback - the view change is silently lost

## Root Cause Analysis

In `core/lib.rs` line 762, each connection clones its own copy of the database schema at creation time:

```rust
let conn = Arc::new(Connection {
    // ...
    schema: RwLock::new(self.schema.lock().clone()),  // <-- Each connection has its own schema copy
    // ...
});
```

When a materialized view is created on connection A:
1. Connection A's local schema is updated with the view definition
2. The dependency mapping (`table_to_materialized_views`) is updated in A's schema
3. Connection B's schema remains unchanged (it was cloned before the view existed)

When an INSERT happens on connection B:
1. `op_insert` looks up dependent views: `schema.get_dependent_materialized_views(table_name)`
2. B's schema returns an empty list (it doesn't know about the view)
3. IVM is skipped entirely

The schema change from `CREATE MATERIALIZED VIEW` does increment the schema_cookie and update the database-level schema, but existing connections don't automatically refresh their local schema copies.

## Impact

This is a silent data loss bug. Applications that:
1. Use connection pooling
2. Create materialized views dynamically
3. Have separate read/write connections

...will experience missing CDC events and incorrect IVM behavior with no error messages.

## Suggested Fix

Options:

### Option 1: Eager schema refresh on write operations
Before any INSERT/UPDATE/DELETE, check if the connection's schema_version is outdated and refresh if needed.

### Option 2: Shared schema reference
Instead of cloning the schema, have all connections reference the same `Arc<RwLock<Schema>>` from the database.

### Option 3: Schema change broadcast
When schema changes (CREATE VIEW, etc.), notify all open connections to refresh their schema.

## Workaround

Create materialized views on the same connection that will perform writes:

```rust
// Instead of:
let read_conn = db.get_connection();
read_conn.execute("CREATE MATERIALIZED VIEW...")?;
let write_conn = db.get_write_connection();
write_conn.execute("INSERT...")?;  // IVM won't fire!

// Do this:
let write_conn = db.get_write_connection();
write_conn.execute("CREATE MATERIALIZED VIEW...")?;  // View in write_conn's schema
write_conn.execute("INSERT...")?;  // IVM fires correctly
```

## Test Case

```rust
#[tokio::test]
async fn test_schema_sync_between_connections() {
    let db = Database::open(":memory:").unwrap();

    // Create table
    let conn_a = db.connect().unwrap();
    conn_a.execute("CREATE TABLE events (id TEXT, status TEXT)").await.unwrap();

    // Create write connection BEFORE the view exists
    let conn_b = db.connect().unwrap();

    // Create view on connection A
    conn_a.execute("CREATE MATERIALIZED VIEW active_events AS SELECT * FROM events WHERE status = 'active'").await.unwrap();

    // Set up CDC callback on connection B
    let received = Arc::new(AtomicBool::new(false));
    let received_clone = received.clone();
    conn_b.set_change_callback(move |event| {
        if event.relation_name == "active_events" {
            received_clone.store(true, Ordering::SeqCst);
        }
    });

    // Insert on connection B - this SHOULD trigger IVM for active_events
    conn_b.execute("INSERT INTO events VALUES ('1', 'active')").await.unwrap();

    // BUG: This assertion fails because conn_b doesn't know about the view
    assert!(received.load(Ordering::SeqCst), "CDC callback should have fired for the view");
}
```

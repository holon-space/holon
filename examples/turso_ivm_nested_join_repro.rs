//! Minimal reproducer for Turso IVM nested JOIN panic
//!
//! This bug occurs when:
//! 1. A materialized view (A) has a JOIN
//! 2. Another materialized view (B) JOINs with (A)
//! 3. CDC callbacks are active via set_view_change_callback()
//! 4. Data is inserted/updated that cascades IVM updates through both views
//!
//! The bug is a re-entrancy issue: JoinOperator::commit is called while
//! a previous commit is still in progress, corrupting cursor state.
//!
//! Run with: cargo run --example turso-ivm-nested-join-repro
//!
//! Expected: Panic with "current_page=-1 is negative"

use turso::Builder;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Create in-memory database with experimental views
    let db = Builder::new_local(":memory:")
        .enable_experimental_views(true)
        .build()?;

    let conn = db.connect()?;

    // Setup base tables
    conn.execute(
        "CREATE TABLE blocks (id TEXT PRIMARY KEY, parent_id TEXT, content TEXT)",
        (),
    )?;
    conn.execute(
        "CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, region TEXT, block_id TEXT)",
        (),
    )?;
    conn.execute(
        "CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER)",
        (),
    )?;
    conn.execute(
        "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
        (),
    )?;

    // Create first matview with JOIN
    conn.execute(
        "CREATE MATERIALIZED VIEW current_focus AS
         SELECT nc.region, nh.block_id
         FROM navigation_cursor nc
         JOIN navigation_history nh ON nc.history_id = nh.id",
        (),
    )?;

    // Create second matview that JOINs with the first (nested)
    conn.execute(
        "CREATE MATERIALIZED VIEW watch_view AS
         SELECT blocks.id, blocks.content
         FROM blocks
         INNER JOIN current_focus cf ON blocks.parent_id = cf.block_id
         WHERE cf.region = 'main'",
        (),
    )?;

    // Set up CDC callback - this is the key trigger for the bug
    // The callback fires during commit, causing re-entrant JoinOperator::commit calls
    conn.set_view_change_callback(|event| {
        println!(
            "CDC callback: {} changes to {}",
            event.changes.len(),
            event.relation_name
        );
    })?;

    println!("Schema created, triggering IVM update...");

    // Insert into navigation_history - this triggers IVM update on current_focus
    conn.execute(
        "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'root-block')",
        (),
    )?;

    // Update cursor to point to the new history entry
    // This triggers cascading IVM updates:
    // 1. current_focus updates (because navigation_cursor changed)
    // 2. watch_view updates (because it JOINs with current_focus)
    // 3. During this cascade, JoinOperator::commit is called re-entrantly
    // 4. PANIC: cursor state corrupted
    println!("Updating navigation_cursor (this should trigger the panic)...");
    conn.execute(
        "UPDATE navigation_cursor SET history_id = 1 WHERE region = 'main'",
        (),
    )?;

    println!("SUCCESS - no panic (bug may be fixed or not triggered)");
    Ok(())
}

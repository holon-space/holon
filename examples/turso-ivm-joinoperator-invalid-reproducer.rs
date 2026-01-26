//! Minimal reproducer for Turso IVM JoinOperator Invalid state panic
//!
//! **Bug**: JoinOperator::commit panics with "Invalid state reached" when:
//! 1. A materialized view with a JOIN exists
//! 2. Data is inserted into a DIFFERENT table that doesn't affect the JOIN view
//! 3. apply_view_deltas is called for ALL views during commit
//! 4. The JoinOperator for the JOIN view is in Invalid state (never received updates)
//!
//! Run with: cargo run --example turso-ivm-joinoperator-invalid-reproducer
//!
//! Expected panic:
//! ```
//! [JoinOperator::commit] Invalid state reached! previous_state=Invalid,
//! left_storage_id=..., right_storage_id=..., input_deltas: left_changes=..., right_changes=0
//! ```

use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Use a fresh database each time
    let db_path = "/tmp/turso-ivm-joinop-invalid.db";
    if Path::new(db_path).exists() {
        std::fs::remove_file(db_path)?;
    }
    // Also remove WAL files
    let _ = std::fs::remove_file(format!("{}-wal", db_path));
    let _ = std::fs::remove_file(format!("{}-shm", db_path));

    println!("=== Turso IVM JoinOperator Invalid State Reproducer ===\n");
    println!("Creating database at {}", db_path);
    let db = turso::Builder::new_local(db_path).build().await?;
    let conn = db.connect()?;

    // =====================================================================
    // STEP 1: Create the events table and its indexes (from TursoEventBus)
    // =====================================================================
    println!("\n[STEP 1] Creating events table with indexes...");

    conn.execute(
        "CREATE TABLE IF NOT EXISTS events (
            id TEXT PRIMARY KEY,
            event_type TEXT NOT NULL,
            aggregate_type TEXT NOT NULL,
            aggregate_id TEXT NOT NULL,
            origin TEXT NOT NULL,
            status TEXT DEFAULT 'confirmed',
            payload TEXT NOT NULL,
            trace_id TEXT,
            command_id TEXT,
            created_at INTEGER NOT NULL,
            processed_by_loro INTEGER DEFAULT 0,
            processed_by_org INTEGER DEFAULT 0,
            processed_by_cache INTEGER DEFAULT 0,
            speculative_id TEXT,
            rejection_reason TEXT
        )",
        (),
    )
    .await?;

    // Create partial indexes (these are what TursoEventBus creates)
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_loro_pending
         ON events(created_at)
         WHERE processed_by_loro = 0 AND origin != 'loro' AND status = 'confirmed'",
        (),
    )
    .await?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_org_pending
         ON events(created_at)
         WHERE processed_by_org = 0 AND origin != 'org' AND status = 'confirmed'",
        (),
    )
    .await?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_cache_pending
         ON events(created_at)
         WHERE processed_by_cache = 0 AND status = 'confirmed'",
        (),
    )
    .await?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_aggregate
         ON events(aggregate_type, aggregate_id, created_at)",
        (),
    )
    .await?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_events_command
         ON events(command_id)
         WHERE command_id IS NOT NULL",
        (),
    )
    .await?;

    println!("  Events table and indexes created");

    // =====================================================================
    // STEP 2: Create a materialized view on events (NO JOIN)
    // =====================================================================
    println!("\n[STEP 2] Creating events_view_block (no JOIN)...");

    conn.execute(
        "CREATE MATERIALIZED VIEW events_view_block AS
         SELECT * FROM events
         WHERE status = 'confirmed' AND aggregate_type = 'block'",
        (),
    )
    .await?;

    println!("  events_view_block created");

    // =====================================================================
    // STEP 3: Create navigation tables (for the JOIN view)
    // =====================================================================
    println!("\n[STEP 3] Creating navigation tables...");

    conn.execute(
        "CREATE TABLE IF NOT EXISTS navigation_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            region TEXT NOT NULL,
            block_id TEXT,
            timestamp TEXT DEFAULT (datetime('now'))
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_navigation_history_region
         ON navigation_history(region)",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS navigation_cursor (
            region TEXT PRIMARY KEY,
            history_id INTEGER REFERENCES navigation_history(id)
        )",
        (),
    )
    .await?;

    // Initialize cursor with NULL history_id
    conn.execute(
        "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
        (),
    )
    .await?;

    println!("  Navigation tables created and initialized");

    // =====================================================================
    // STEP 4: Create current_focus materialized view (HAS A JOIN!)
    // =====================================================================
    println!("\n[STEP 4] Creating current_focus view (WITH JOIN)...");

    // Drop first in case it exists
    let _ = conn.execute("DROP VIEW IF EXISTS current_focus", ()).await;

    conn.execute(
        "CREATE MATERIALIZED VIEW current_focus AS
         SELECT
             nc.region,
             nh.block_id,
             nh.timestamp
         FROM navigation_cursor nc
         JOIN navigation_history nh ON nc.history_id = nh.id",
        (),
    )
    .await?;

    println!("  current_focus created (this view has a JOIN)");

    // =====================================================================
    // STEP 5: INSERT into events table
    //
    // This is where the bug manifests:
    // - events table is NOT part of current_focus view
    // - But apply_view_deltas is called for ALL views during commit
    // - The JoinOperator for current_focus is in Invalid state
    //   because it never received any updates (events doesn't affect it)
    // =====================================================================
    println!("\n[STEP 5] Inserting into events table...");
    println!("  NOTE: events table is NOT part of current_focus JOIN view");
    println!("  This should trigger the JoinOperator Invalid state panic...\n");

    let insert_result = conn
        .execute(
            "INSERT INTO events (
                id, event_type, aggregate_type, aggregate_id, origin,
                status, payload, created_at
            ) VALUES (
                '01KE1Z0Y3YDVGJ6WK0TESTEVT1',
                'block_created',
                'block',
                'd153ac2e-64b6-4b98-8c92-eb82f0c9e123',
                'loro',
                'confirmed',
                '{\"type\":\"block_created\",\"block_id\":\"test-block\"}',
                1735909133978
            )",
            (),
        )
        .await;

    match insert_result {
        Ok(_) => {
            println!("  INSERT succeeded (bug did not reproduce)");
            println!("\n  This might mean:");
            println!("  - Turso fixed the bug");
            println!("  - The reproducer needs adjustment");
            println!("  - The issue requires more complex state");
        }
        Err(e) => {
            println!("  INSERT failed with error: {}", e);
            if e.to_string().contains("Invalid state") {
                println!("\n  BUG REPRODUCED: JoinOperator Invalid state panic!");
            }
        }
    }

    println!("\n=== Test complete ===");
    Ok(())
}

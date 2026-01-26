//! Minimal reproducer for Turso IVM join panics
//!
//! Run with: cargo run --example turso-ivm-minimal-reproducer
//!
//! This reproduces panics in Turso's incremental view system when:
//! 1. Creating materialized views with JOINs
//! 2. After failed view creation attempts leave corrupted state

use std::path::Path;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Use a fresh database each time
    let db_path = "/tmp/turso-ivm-test.db";
    if Path::new(db_path).exists() {
        std::fs::remove_file(db_path)?;
    }
    // Also remove WAL files
    let _ = std::fs::remove_file(format!("{}-wal", db_path));
    let _ = std::fs::remove_file(format!("{}-shm", db_path));

    println!("Creating database at {}", db_path);
    let db = turso::Builder::new_local(db_path).build().await?;
    let conn = db.connect()?;

    println!("Creating tables...");
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
        "CREATE TABLE IF NOT EXISTS navigation_cursor (
            region TEXT PRIMARY KEY,
            history_id INTEGER REFERENCES navigation_history(id)
        )",
        (),
    )
    .await?;

    println!("Tables created successfully");

    // Insert some test data first (empty tables might be the issue?)
    println!("Inserting test data...");
    conn.execute(
        "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
        (),
    )
    .await?;

    println!("Creating materialized view with JOIN...");
    // This is the view that panics
    let result = conn
        .execute(
            "CREATE MATERIALIZED VIEW current_focus AS
            SELECT
                nc.region,
                nh.block_id,
                nh.timestamp
            FROM navigation_cursor nc
            JOIN navigation_history nh ON nc.history_id = nh.id",
            (),
        )
        .await;

    match result {
        Ok(_) => println!("Materialized view created successfully!"),
        Err(e) => println!("Error creating materialized view: {}", e),
    }

    println!("Done");
    Ok(())
}

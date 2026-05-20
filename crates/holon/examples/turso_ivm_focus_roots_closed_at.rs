//! Does `UPDATE navigation_history SET closed_at = <ts>` evict a row from the
//! `focus_roots` matview?
//!
//! This is the EXACT production close path (`navigation/provider.rs::focus()`
//! → `close_open_in_region.sql`): the matview filters `closed_at IS NULL`, and
//! navigating away flips `closed_at` from NULL to a timestamp on the prior
//! open row. If the IVM doesn't propagate that NULL→value transition as a
//! matview delete, the stale row lingers — which is the "`block:journals`
//! appears when it shouldn't" symptom the PBT blames on Turso.
//!
//! The companion `turso_ivm_focus_roots_null_filter` repro only ever tests the
//! `block_id` filter column (value→NULL). It never touches `closed_at`, so the
//! production eviction path is unverified. This repro closes that gap.
//!
//! Run: cargo run --example turso_ivm_focus_roots_closed_at -p holon

use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("=== focus_roots: UPDATE closed_at evicts the row? ===\n");

    let mut all_passed = true;
    all_passed &= test_close_single_open_row().await?;
    all_passed &= test_production_focus_replace_sequence().await?;

    println!("\n{}", "=".repeat(60));
    if all_passed {
        println!("ALL CHECKS PASSED — `UPDATE closed_at = <ts>` correctly evicts the");
        println!("row from focus_roots at the matview level. The lingering");
        println!("`block:journals` focus root is NOT a Turso IVM eviction bug — look");
        println!("at the holon close path / reference model instead.");
    } else {
        println!("SOME CHECKS FAILED — Turso IVM does NOT evict on the closed_at");
        println!("transition; the `block:journals` drift IS upstream. File it.");
        std::process::exit(1);
    }

    Ok(())
}

/// Insert one open row, flip its `closed_at`, expect the matview to empty.
async fn test_close_single_open_row() -> anyhow::Result<bool> {
    println!("--- Test 1: single open row, UPDATE closed_at = ts ---");
    let db = fresh_db("closedat-test1").await?;
    let conn = db.connect()?;
    create_schema(&conn).await?;

    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) \
         VALUES ('main', 'block:journals', 1000)",
        (),
    )
    .await?;

    let before = count(&conn, "SELECT COUNT(*) FROM focus_roots").await?;
    println!("  Before close: focus_roots has {before} rows (expected: 1)");

    conn.execute(
        "UPDATE navigation_history SET closed_at = datetime('now') \
         WHERE region = 'main' AND closed_at IS NULL",
        (),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let base_open = count(
        &conn,
        "SELECT COUNT(*) FROM navigation_history WHERE closed_at IS NULL",
    )
    .await?;
    let after = count(&conn, "SELECT COUNT(*) FROM focus_roots").await?;
    println!(
        "  After close:  base table open rows = {base_open} (expected: 0), \
         focus_roots = {after} (expected: 0)"
    );

    check(
        before == 1 && base_open == 0 && after == 0,
        "UPDATE closed_at evicts the single row",
    )
}

/// Replicate the production focus-replace sequence verbatim: open `journals`,
/// then "navigate" to `block:page-a` by (1) closing all open rows in the
/// region, (2) inserting a fresh open row. The matview must converge to
/// exactly `{block:page-a}` — `block:journals` must be gone.
async fn test_production_focus_replace_sequence() -> anyhow::Result<bool> {
    println!("--- Test 2: production focus-replace (journals → page-a) ---");
    let db = fresh_db("closedat-test2").await?;
    let conn = db.connect()?;
    create_schema(&conn).await?;

    // Startup: focus journals in main.
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) \
         VALUES ('main', 'block:journals', 1000)",
        (),
    )
    .await?;

    // Navigate to page-a: close prior open, then insert new open.
    conn.execute(
        "UPDATE navigation_history SET closed_at = datetime('now') \
         WHERE region = 'main' AND closed_at IS NULL",
        (),
    )
    .await?;
    conn.execute(
        "INSERT INTO navigation_history (region, block_id, timestamp) \
         VALUES ('main', 'block:page-a', 1001)",
        (),
    )
    .await?;
    tokio::time::sleep(Duration::from_millis(100)).await;

    let roots = root_ids(&conn, "main").await?;
    println!("  focus_roots(main) = {roots:?} (expected: [\"block:page-a\"])");
    let journals_lingers = roots.iter().any(|r| r == "block:journals");
    if journals_lingers {
        println!("  >>> block:journals LINGERS in the matview after close <<<");
    }

    check(
        roots == vec!["block:page-a".to_string()],
        "focus-replace converges to the new page only",
    )
}

async fn create_schema(conn: &turso::Connection) -> anyhow::Result<()> {
    conn.execute(
        "CREATE TABLE navigation_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            region TEXT NOT NULL,
            block_id TEXT,
            timestamp INTEGER NOT NULL,
            closed_at TEXT
        )",
        (),
    )
    .await?;
    // Mirrors crates/holon/sql/schema/matview_focus_roots.sql exactly.
    conn.execute(
        "CREATE MATERIALIZED VIEW focus_roots AS
         SELECT
             region,
             block_id AS root_id,
             timestamp AS added_ts,
             id AS history_id
         FROM navigation_history
         WHERE closed_at IS NULL AND block_id IS NOT NULL",
        (),
    )
    .await?;
    Ok(())
}

async fn root_ids(conn: &turso::Connection, region: &str) -> anyhow::Result<Vec<String>> {
    let mut rows = conn
        .query(
            "SELECT root_id FROM focus_roots WHERE region = ?1 ORDER BY root_id",
            turso::params![region],
        )
        .await?;
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        if let turso::Value::Text(s) = row.get_value(0)? {
            out.push(s);
        }
    }
    Ok(out)
}

async fn count(conn: &turso::Connection, sql: &str) -> anyhow::Result<i64> {
    let mut rows = conn.query(sql, ()).await?;
    let row = rows.next().await?.expect("count query returns a row");
    Ok(row.get::<i64>(0)?)
}

fn check(passed: bool, label: &str) -> anyhow::Result<bool> {
    println!("  [{}] {label}\n", if passed { "PASS" } else { "FAIL" });
    Ok(passed)
}

async fn fresh_db(name: &str) -> anyhow::Result<turso::Database> {
    let db_path = format!("/tmp/{name}.db");
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{suffix}"));
    }
    Ok(turso::Builder::new_local(&db_path)
        .experimental_materialized_views(true)
        .build()
        .await?)
}

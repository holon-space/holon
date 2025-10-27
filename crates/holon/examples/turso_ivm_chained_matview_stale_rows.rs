//! Reproducer for Turso IVM: chained matview retains stale rows after upstream UPDATE
//!
//! Bug: When a materialized view (MV-B) depends on another matview (MV-A),
//! and MV-A's source table is UPDATEd (changing the join key), MV-A updates
//! correctly but MV-B retains stale rows from the previous MV-A state.
//!
//! Schema (from production):
//!   navigation_cursor (table) → current_focus (matview, JOIN) → focus_roots (matview, JOIN+UNION)
//!   items table is also joined into focus_roots
//!
//! The bug manifests when CDC callbacks are active and concurrent mutations
//! trigger IVM cascades through multiple matview chains simultaneously.
//!
//! Observed in production:
//!   - current_focus: 1 row (region=main, block_id=doc:bbb) ✓
//!   - focus_roots: 7 rows, including 2 from doc:aaa (previous navigation) ✗
//!   - Raw SQL re-evaluation: 8 rows, all from doc:bbb ✓
//!
//! Run with: cargo run --example turso_ivm_chained_matview_stale_rows

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_path = "/tmp/turso-ivm-chained-stale-rows.db";
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }

    println!("=== Turso IVM: Chained Matview Stale Rows Reproducer ===\n");

    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;

    // =====================================================================
    // STEP 1: Create production-like schema
    // =====================================================================
    println!("[STEP 1] Creating tables...");

    conn.execute(
        "CREATE TABLE items (
            id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL,
            content TEXT DEFAULT '',
            content_type TEXT DEFAULT 'text',
            source_language TEXT,
            properties TEXT DEFAULT '{}'
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TABLE navigation_history (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            region TEXT NOT NULL,
            block_id TEXT,
            timestamp TEXT DEFAULT (datetime('now'))
        )",
        (),
    )
    .await?;

    conn.execute(
        "CREATE INDEX idx_nav_history_region ON navigation_history(region)",
        (),
    )
    .await?;

    conn.execute(
        "CREATE TABLE navigation_cursor (
            region TEXT PRIMARY KEY,
            history_id INTEGER REFERENCES navigation_history(id)
        )",
        (),
    )
    .await?;

    conn.execute(
        "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', NULL)",
        (),
    )
    .await?;

    // =====================================================================
    // STEP 2: Create chained matviews (production schema)
    // =====================================================================
    println!("[STEP 2] Creating materialized views...");

    // Extra matview on items (like blocks_with_paths in production)
    // This creates IVM pressure on the items table from multiple directions
    conn.execute(
        "CREATE MATERIALIZED VIEW items_with_paths AS
        WITH RECURSIVE paths AS (
            SELECT id, parent_id, content, content_type,
                   '/' || id as path
            FROM items
            WHERE parent_id LIKE 'doc:%'
            UNION ALL
            SELECT i.id, i.parent_id, i.content, i.content_type,
                   p.path || '/' || i.id as path
            FROM items i
            INNER JOIN paths p ON i.parent_id = p.id
        )
        SELECT * FROM paths",
        (),
    )
    .await?;
    println!("  items_with_paths created (recursive CTE)");

    // MV-A: current_focus
    conn.execute(
        "CREATE MATERIALIZED VIEW current_focus AS
         SELECT nc.region, nh.block_id, nh.timestamp
         FROM navigation_cursor nc
         JOIN navigation_history nh ON nc.history_id = nh.id",
        (),
    )
    .await?;
    println!("  current_focus created (MV-A)");

    // MV-B: focus_roots (chained: depends on MV-A + items table)
    conn.execute(
        "CREATE MATERIALIZED VIEW focus_roots AS
         SELECT cf.region, cf.block_id, i.id AS root_id
         FROM current_focus AS cf
         JOIN items AS i ON i.parent_id = cf.block_id
         UNION ALL
         SELECT cf.region, cf.block_id, i.id AS root_id
         FROM current_focus AS cf
         JOIN items AS i ON i.id = cf.block_id",
        (),
    )
    .await?;
    println!("  focus_roots created (MV-B, chained)");

    // =====================================================================
    // STEP 3: Set up CDC callback (critical for reproducing)
    // =====================================================================
    println!("[STEP 3] Setting up CDC callbacks...");

    let cdc_count = Arc::new(AtomicUsize::new(0));
    let cdc_count_clone = cdc_count.clone();
    conn.set_change_callback(move |event| {
        cdc_count_clone.fetch_add(1, Ordering::SeqCst);
        println!(
            "  CDC #{}: {} changes to {}",
            cdc_count_clone.load(Ordering::SeqCst),
            event.changes.len(),
            event.relation_name
        );
    })?;

    // =====================================================================
    // STEP 4: Insert items for multiple documents (realistic data volume)
    // =====================================================================
    println!("\n[STEP 4] Inserting items...");

    // Document A: 8 root items + 4 children (like a project document)
    for i in 1..=8 {
        conn.execute(
            &format!(
                "INSERT INTO items (id, parent_id, content) VALUES ('a-root-{i}', 'doc:aaa', 'Doc A root {i}')"
            ),
            (),
        )
        .await?;
    }
    for i in 1..=4 {
        conn.execute(
            &format!(
                "INSERT INTO items (id, parent_id, content) VALUES ('a-child-{i}', 'a-root-1', 'Doc A child {i}')"
            ),
            (),
        )
        .await?;
    }
    println!("  12 items under doc:aaa (8 roots + 4 children)");

    // Document B: 5 root items + 3 children
    for i in 1..=5 {
        conn.execute(
            &format!(
                "INSERT INTO items (id, parent_id, content) VALUES ('b-root-{i}', 'doc:bbb', 'Doc B root {i}')"
            ),
            (),
        )
        .await?;
    }
    for i in 1..=3 {
        conn.execute(
            &format!(
                "INSERT INTO items (id, parent_id, content) VALUES ('b-child-{i}', 'b-root-1', 'Doc B child {i}')"
            ),
            (),
        )
        .await?;
    }
    println!("  8 items under doc:bbb (5 roots + 3 children)");

    // Document C: 3 items
    for i in 1..=3 {
        conn.execute(
            &format!(
                "INSERT INTO items (id, parent_id, content) VALUES ('c-root-{i}', 'doc:ccc', 'Doc C root {i}')"
            ),
            (),
        )
        .await?;
    }
    println!("  3 items under doc:ccc");

    // =====================================================================
    // STEP 5: Navigate through multiple documents (stress test)
    // =====================================================================
    let docs = [
        ("doc:aaa", 8), // 8 direct children
        ("doc:bbb", 5), // 5 direct children
        ("doc:ccc", 3), // 3 direct children
        ("doc:bbb", 5), // back to B
        ("doc:aaa", 8), // back to A
        ("doc:ccc", 3), // to C
    ];

    for (nav_idx, (doc_id, expected_roots)) in docs.iter().enumerate() {
        let history_id = nav_idx + 1;
        println!("\n[NAV {history_id}] Navigating to {doc_id}...");

        conn.execute(
            &format!(
                "INSERT INTO navigation_history (region, block_id) VALUES ('main', '{doc_id}')"
            ),
            (),
        )
        .await?;
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', {history_id})"
            ),
            (),
        )
        .await?;

        // Simulate concurrent item mutations during navigation
        // (org sync inserting blocks while user navigates)
        conn.execute(
            &format!(
                "INSERT OR REPLACE INTO items (id, parent_id, content) VALUES ('ephemeral-{history_id}', '{doc_id}', 'Concurrent mutation {history_id}')"
            ),
            (),
        )
        .await?;

        // Check for staleness
        let cf_block_id = get_string(
            &conn,
            "SELECT block_id FROM current_focus WHERE region = 'main'",
        )
        .await?;
        let mv_count = count_query(
            &conn,
            "SELECT count(*) as cnt FROM focus_roots WHERE region = 'main'",
        )
        .await?;
        let stale_count = count_query(
            &conn,
            &format!("SELECT count(*) as cnt FROM focus_roots WHERE region = 'main' AND block_id != '{doc_id}'"),
        ).await?;

        // Clean up ephemeral item
        conn.execute(
            &format!("DELETE FROM items WHERE id = 'ephemeral-{history_id}'"),
            (),
        )
        .await?;

        let expected = *expected_roots;
        println!("  current_focus.block_id = {cf_block_id}");
        println!("  focus_roots count = {mv_count} (expected ~{expected})");
        println!("  stale rows = {stale_count}");

        if stale_count > 0 {
            println!("\n=== BUG REPRODUCED at navigation step {history_id}! ===");
            println!("  current_focus correctly shows: {doc_id}");
            println!("  focus_roots has {stale_count} stale rows from previous navigation");
            print_query(
                &conn,
                "focus_roots contents",
                "SELECT region, block_id, root_id FROM focus_roots WHERE region = 'main' ORDER BY root_id",
            )
            .await?;

            let raw_count = count_query(
                &conn,
                &format!(
                    "SELECT count(*) as cnt FROM (
                    SELECT cf.region, cf.block_id, i.id AS root_id
                    FROM current_focus AS cf
                    JOIN items AS i ON i.parent_id = cf.block_id
                    UNION ALL
                    SELECT cf.region, cf.block_id, i.id AS root_id
                    FROM current_focus AS cf
                    JOIN items AS i ON i.id = cf.block_id
                ) WHERE region = 'main'"
                ),
            )
            .await?;
            println!("  Raw SQL re-evaluation: {raw_count} rows (no stale)");

            std::process::exit(1);
        }
    }

    println!("\n=== VERDICT ===");
    println!("Bug did NOT reproduce in {} navigation cycles.", docs.len());
    println!("The bug may require:");
    println!("  - Higher concurrency (multiple connections)");
    println!("  - More matviews in the IVM cascade");
    println!("  - Specific timing of CDC callback processing");
    println!("  - Longer-running process (accumulated state drift)");
    println!("\nTotal CDC events: {}", cdc_count.load(Ordering::SeqCst));

    Ok(())
}

async fn count_query(conn: &turso::Connection, sql: &str) -> anyhow::Result<i64> {
    let mut rows = conn.query(sql, ()).await?;
    if let Some(row) = rows.next().await? {
        let count: i64 = row.get(0)?;
        Ok(count)
    } else {
        Ok(0)
    }
}

async fn get_string(conn: &turso::Connection, sql: &str) -> anyhow::Result<String> {
    let mut rows = conn.query(sql, ()).await?;
    if let Some(row) = rows.next().await? {
        let val: String = row.get(0)?;
        Ok(val)
    } else {
        Ok("<no rows>".to_string())
    }
}

async fn print_query(conn: &turso::Connection, label: &str, sql: &str) -> anyhow::Result<()> {
    println!("  {label}:");
    let mut rows = conn.query(sql, ()).await?;
    while let Some(row) = rows.next().await? {
        let mut parts = Vec::new();
        for i in 0..10 {
            match row.get::<String>(i) {
                Ok(val) => parts.push(val),
                Err(_) => break,
            }
        }
        println!("    {}", parts.join(" | "));
    }
    Ok(())
}

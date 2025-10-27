//! Reproducer for Turso IVM: 3-level matview chain drops CDC
//!
//! Production scenario:
//!   Level 0 (tables): navigation_cursor, navigation_history, block
//!   Level 1 (matview): current_focus = cursor JOIN history
//!   Level 2 (matview): focus_roots = cursor JOIN history JOIN block (inlined current_focus)
//!   Level 3 (matview): watch_view = focus_roots JOIN block (recursive CTE)
//!
//! After updating navigation_cursor:
//!   - current_focus updates correctly (level 1)
//!   - focus_roots updates correctly (level 2, after inlining fix)
//!   - watch_view does NOT receive CDC (level 3 — matview-on-matview)
//!
//! The UI subscribes to CDC on watch_view, so navigation changes are invisible.
//!
//! Run with: cargo run --example turso_ivm_3level_chain_cdc

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let db_path = "/tmp/turso-ivm-3level-chain.db";
    for ext in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{db_path}{ext}"));
    }

    println!("=== Turso IVM: 3-Level Matview Chain CDC Reproducer ===\n");

    let db = turso::Builder::new_local(db_path)
        .experimental_materialized_views(true)
        .build()
        .await?;
    let conn = db.connect()?;

    // =====================================================================
    // STEP 1: Create base tables
    // =====================================================================
    println!("[STEP 1] Creating tables...");

    conn.execute(
        "CREATE TABLE block (
            id TEXT PRIMARY KEY,
            parent_id TEXT NOT NULL,
            content TEXT DEFAULT '',
            content_type TEXT DEFAULT 'text',
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
    // STEP 2: Insert blocks for two documents
    // =====================================================================
    println!("[STEP 2] Inserting blocks...");

    for i in 1..=5 {
        conn.execute(
            &format!(
                "INSERT INTO block (id, parent_id, content, content_type) \
                 VALUES ('a-{i}', 'doc:aaa', 'Doc A block {i}', 'text')"
            ),
            (),
        )
        .await?;
    }
    println!("  5 blocks under doc:aaa");

    for i in 1..=3 {
        conn.execute(
            &format!(
                "INSERT INTO block (id, parent_id, content, content_type) \
                 VALUES ('b-{i}', 'doc:bbb', 'Doc B block {i}', 'text')"
            ),
            (),
        )
        .await?;
    }
    println!("  3 blocks under doc:bbb");

    // =====================================================================
    // STEP 3: Create matview chain (matching production after inlining fix)
    // =====================================================================
    println!("[STEP 3] Creating materialized views...");

    // Level 1: current_focus (for direct queries, not chained into)
    conn.execute(
        "CREATE MATERIALIZED VIEW current_focus AS
         SELECT nc.region, nh.block_id, nh.timestamp
         FROM navigation_cursor nc
         JOIN navigation_history nh ON nc.history_id = nh.id",
        (),
    )
    .await?;
    println!("  Level 1: current_focus (cursor JOIN history)");

    // Level 2: focus_roots (inlined — reads base tables, not current_focus)
    conn.execute(
        "CREATE MATERIALIZED VIEW focus_roots AS
         SELECT nc.region, nh.block_id, b.id AS root_id
         FROM navigation_cursor nc
         JOIN navigation_history nh ON nc.history_id = nh.id
         JOIN block AS b ON b.parent_id = nh.block_id
         UNION ALL
         SELECT nc.region, nh.block_id, b.id AS root_id
         FROM navigation_cursor nc
         JOIN navigation_history nh ON nc.history_id = nh.id
         JOIN block AS b ON b.id = nh.block_id",
        (),
    )
    .await?;
    println!("  Level 2: focus_roots (inlined cursor JOIN history JOIN block)");

    // NOTE: watch_view is created LATER (after first navigation) to match production.
    // In production, query_and_watch lazily creates the matview when the UI first renders.

    // =====================================================================
    // STEP 4: Set up CDC callback
    // =====================================================================
    println!("\n[STEP 4] Setting up CDC...");

    let watch_view_events = Arc::new(AtomicUsize::new(0));
    let focus_roots_events = Arc::new(AtomicUsize::new(0));
    let current_focus_events = Arc::new(AtomicUsize::new(0));

    let wv = watch_view_events.clone();
    let fr = focus_roots_events.clone();
    let cf = current_focus_events.clone();

    conn.set_change_callback(move |event| {
        let name = &event.relation_name;
        let count = event.changes.len();
        if name.starts_with("watch_view") {
            wv.fetch_add(count, Ordering::SeqCst);
            println!("  CDC: {} changes to watch_view", count);
        } else if name.starts_with("focus_roots") {
            fr.fetch_add(count, Ordering::SeqCst);
            println!("  CDC: {} changes to focus_roots", count);
        } else if name.starts_with("current_focus") {
            cf.fetch_add(count, Ordering::SeqCst);
            println!("  CDC: {} changes to current_focus", count);
        }
    })?;

    // =====================================================================
    // STEP 5: Navigate to doc:aaa, then create watch_view (production order)
    // =====================================================================
    println!("\n[STEP 5] Navigate to doc:aaa...");

    conn.execute(
        "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:aaa')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 1)",
        (),
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let fr_count = count_query(
        &conn,
        "SELECT count(*) as cnt FROM focus_roots WHERE region = 'main'",
    )
    .await?;
    println!("  focus_roots: {} rows after navigation", fr_count);
    assert!(
        fr_count > 0,
        "focus_roots should have rows after navigation"
    );

    // Create blocks_with_paths (recursive CTE, creates IVM pressure like production)
    conn.execute(
        "CREATE MATERIALIZED VIEW blocks_with_paths AS
         WITH RECURSIVE paths AS (
             SELECT id, parent_id, content, content_type,
                    '/' || id as path
             FROM block
             WHERE parent_id LIKE 'doc:%'
             UNION ALL
             SELECT b.id, b.parent_id, b.content, b.content_type,
                    p.path || '/' || b.id as path
             FROM block b
             INNER JOIN paths p ON b.parent_id = p.id
         )
         SELECT * FROM paths",
        (),
    )
    .await?;
    println!("  Created blocks_with_paths (IVM pressure)");

    // Create structural watch matviews for each block (like UiWatcher does)
    // These watch `block` table changes — adding IVM pressure
    for suffix in [
        "struct_root",
        "struct_main",
        "struct_left",
        "struct_right",
        "struct_a",
        "struct_b",
        "struct_c",
        "struct_d",
    ] {
        conn.execute(
            &format!(
                "CREATE MATERIALIZED VIEW watch_view_{suffix} AS \
                 SELECT id, content, content_type, parent_id FROM block \
                 WHERE id = 'dummy-{suffix}' OR parent_id = 'dummy-{suffix}'"
            ),
            (),
        )
        .await?;
    }
    println!("  Created 8 structural watch matviews (IVM pressure)");

    // NOW create the main watch_view — this matches production where query_and_watch
    // lazily creates the matview AFTER focus_roots already has data and many other
    // matviews exist.
    println!("  Creating watch_view AFTER focus_roots has data (production order)...");
    conn.execute(
        "CREATE MATERIALIZED VIEW watch_view AS
         SELECT b.id, b.parent_id, b.content, b.content_type
         FROM focus_roots fr
         JOIN block b ON b.id = fr.root_id
         WHERE fr.region = 'main'",
        (),
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(100)).await;

    let wv_count = count_query(&conn, "SELECT count(*) as cnt FROM watch_view").await?;
    let wv_cdc = watch_view_events.load(Ordering::SeqCst);
    let fr_cdc = focus_roots_events.load(Ordering::SeqCst);

    println!("  watch_view:  {} rows, {} CDC events", wv_count, wv_cdc);
    println!("  focus_roots CDC: {}", fr_cdc);

    // =====================================================================
    // STEP 6: Navigate to doc:bbb (the critical test)
    // =====================================================================
    println!("\n[STEP 6] Navigate to doc:bbb...");

    // Reset CDC counters
    watch_view_events.store(0, Ordering::SeqCst);
    focus_roots_events.store(0, Ordering::SeqCst);
    current_focus_events.store(0, Ordering::SeqCst);

    conn.execute(
        "INSERT INTO navigation_history (region, block_id) VALUES ('main', 'doc:bbb')",
        (),
    )
    .await?;
    conn.execute(
        "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 2)",
        (),
    )
    .await?;

    tokio::time::sleep(Duration::from_millis(200)).await;

    let cf_block = get_string(
        &conn,
        "SELECT block_id FROM current_focus WHERE region = 'main'",
    )
    .await?;
    let fr_count = count_query(
        &conn,
        "SELECT count(*) as cnt FROM focus_roots WHERE region = 'main'",
    )
    .await?;
    let fr_block = get_string(
        &conn,
        "SELECT block_id FROM focus_roots WHERE region = 'main' LIMIT 1",
    )
    .await?;
    let wv_count = count_query(&conn, "SELECT count(*) as cnt FROM watch_view").await?;

    // Re-evaluate via simpler query (recursive CTEs not supported at query time)
    let raw_wv_count = count_query(
        &conn,
        "SELECT count(*) as cnt FROM (
            SELECT b.id
            FROM focus_roots fr
            JOIN block b ON b.id = fr.root_id
            WHERE fr.region = 'main'
        )",
    )
    .await?;

    let wv_cdc = watch_view_events.load(Ordering::SeqCst);
    let fr_cdc = focus_roots_events.load(Ordering::SeqCst);
    let cf_cdc = current_focus_events.load(Ordering::SeqCst);

    println!("\n=== RESULTS after navigating to doc:bbb ===");
    println!("  current_focus.block_id = {cf_block}");
    println!("  focus_roots: {fr_count} rows, block_id={fr_block}");
    println!("  watch_view (matview):  {wv_count} rows");
    println!("  watch_view (raw SQL):  {raw_wv_count} rows");
    println!("  CDC events: current_focus={cf_cdc}, focus_roots={fr_cdc}, watch_view={wv_cdc}");

    // Check for staleness
    let focus_roots_stale = fr_block != "doc:bbb";
    let watch_view_stale = wv_count != raw_wv_count;
    let watch_view_no_cdc = wv_cdc == 0;

    if focus_roots_stale {
        println!("\n!!! BUG: focus_roots is stale (shows {fr_block} instead of doc:bbb)");
    }
    if watch_view_stale {
        println!(
            "\n!!! BUG: watch_view matview ({wv_count} rows) disagrees with \
             raw SQL re-evaluation ({raw_wv_count} rows)"
        );
    }
    if watch_view_no_cdc && !focus_roots_stale {
        println!(
            "\n!!! BUG: watch_view received 0 CDC events even though focus_roots \
             updated ({fr_cdc} CDC events). The matview-on-matview chain dropped CDC."
        );
    }

    if !focus_roots_stale && !watch_view_stale && !watch_view_no_cdc {
        println!("\n=== All checks passed — bug did NOT reproduce ===");
    }

    // Assert for CI
    assert!(
        !focus_roots_stale,
        "focus_roots should show doc:bbb, got {fr_block}"
    );
    assert!(
        wv_cdc > 0,
        "watch_view should receive CDC events when focus_roots changes, got 0. \
         focus_roots got {fr_cdc} CDC events."
    );

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

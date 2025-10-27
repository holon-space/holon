//! Minimal reproducer: Turso IVM CDC missing for chained UNION ALL matview after INSERT
//!
//! Scenario (mirrors production `navigation.sql`):
//!   1. Tables: navigation_history, navigation_cursor, block
//!   2. Matview chain: current_focus (joins cursor+history) → focus_roots (joins current_focus+block)
//!   3. Navigate to focus on block 'b1'
//!   4. INSERT a new block 'b2' with parent_id='b1'
//!   5. Expected: focus_roots CDC fires (b2 is a child of focus target)
//!   6. Actual: No CDC event for focus_roots
//!
//! Run with:
//!   cargo test -p holon turso_ivm_union_all_insert_repro -- --nocapture

use super::turso::TursoBackend;
use tempfile::TempDir;

/// Simple case: focus_roots with direct table (no chain) — CDC works
#[tokio::test]
async fn union_all_matview_cdc_after_insert_simple() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("repro_simple.db");

    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS focus (region TEXT NOT NULL, block_id TEXT NOT NULL)",
        )
        .await
        .unwrap();

    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS block (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL DEFAULT '',
                parent_id TEXT NOT NULL DEFAULT ''
            )",
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO focus (region, block_id) VALUES ('left', 'b1')",
            vec![],
        )
        .await
        .unwrap();
    handle
        .execute(
            "INSERT INTO block (id, content, parent_id) VALUES ('b1', 'parent', 'doc:root')",
            vec![],
        )
        .await
        .unwrap();

    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS mv_simple AS
             SELECT f.region, f.block_id, b.id AS root_id
             FROM focus f JOIN block b ON b.parent_id = f.block_id
             UNION ALL
             SELECT f.region, f.block_id, b.id AS root_id
             FROM focus f JOIN block b ON b.id = f.block_id",
        )
        .await
        .unwrap();

    // Drain setup events
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    while cdc_rx.try_recv().is_ok() {}

    // INSERT child
    handle
        .execute(
            "INSERT INTO block (id, content, parent_id) VALUES ('b2', 'child', 'b1')",
            vec![],
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let mut events = 0;
    while let Ok(batch) = cdc_rx.try_recv() {
        if batch.metadata.relation_name.starts_with("mv_simple") {
            events += batch.inner.items.len();
        }
    }
    eprintln!("[simple] CDC events for mv_simple: {}", events);
    assert!(
        events > 0,
        "Simple case: CDC should fire for UNION ALL matview on INSERT"
    );
}

/// Chained case: focus_roots depends on current_focus matview — CDC may be missing
#[tokio::test]
async fn union_all_matview_cdc_after_insert_chained() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("repro_chained.db");

    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    // Create tables (matching navigation.sql)
    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS navigation_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                region TEXT NOT NULL,
                block_id TEXT,
                timestamp TEXT DEFAULT (datetime('now'))
            )",
        )
        .await
        .unwrap();

    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS navigation_cursor (
                region TEXT PRIMARY KEY,
                history_id INTEGER REFERENCES navigation_history(id)
            )",
        )
        .await
        .unwrap();

    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS block (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL DEFAULT '',
                parent_id TEXT NOT NULL DEFAULT ''
            )",
        )
        .await
        .unwrap();

    // Insert block b1 and set up navigation focus on it
    handle
        .execute(
            "INSERT INTO block (id, content, parent_id) VALUES ('b1', 'parent', 'doc:root')",
            vec![],
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO navigation_history (id, region, block_id) VALUES (1, 'left_sidebar', 'b1')",
            vec![],
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO navigation_cursor (region, history_id) VALUES ('left_sidebar', 1)",
            vec![],
        )
        .await
        .unwrap();

    // Create matview chain (matching navigation.sql)
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW current_focus AS
             SELECT nc.region, nh.block_id, nh.timestamp
             FROM navigation_cursor nc
             JOIN navigation_history nh ON nc.history_id = nh.id",
        )
        .await
        .unwrap();

    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW focus_roots AS
             SELECT cf.region, cf.block_id, b.id AS root_id
             FROM current_focus AS cf
             JOIN block AS b ON b.parent_id = cf.block_id
             UNION ALL
             SELECT cf.region, cf.block_id, b.id AS root_id
             FROM current_focus AS cf
             JOIN block AS b ON b.id = cf.block_id",
        )
        .await
        .unwrap();

    // Drain setup events
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    while cdc_rx.try_recv().is_ok() {}

    // Verify initial state
    let initial = handle
        .query(
            "SELECT root_id FROM focus_roots ORDER BY root_id",
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    let initial_ids: Vec<String> = initial
        .iter()
        .filter_map(|r| {
            r.get("root_id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect();
    eprintln!("[chained] Initial focus_roots: {:?}", initial_ids);
    assert_eq!(initial_ids, vec!["b1"]);

    // INSERT a child block (parent_id matches focus target)
    handle
        .execute(
            "INSERT INTO block (id, content, parent_id) VALUES ('b2', 'child', 'b1')",
            vec![],
        )
        .await
        .unwrap();
    eprintln!("[chained] Inserted child block b2 with parent_id=b1");

    // Wait for CDC
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let mut focus_roots_events = 0;
    let mut current_focus_events = 0;
    let mut block_events = 0;
    while let Ok(batch) = cdc_rx.try_recv() {
        let name = &batch.metadata.relation_name;
        let count = batch.inner.items.len();
        eprintln!("[chained] CDC: relation='{}' items={}", name, count);
        if name.starts_with("focus_roots") {
            focus_roots_events += count;
        } else if name.starts_with("current_focus") {
            current_focus_events += count;
        } else if name.starts_with("block") {
            block_events += count;
        }
    }
    eprintln!(
        "[chained] CDC summary: focus_roots={}, current_focus={}, block={}",
        focus_roots_events, current_focus_events, block_events
    );

    // Verify matview query returns both rows
    let after = handle
        .query(
            "SELECT root_id FROM focus_roots ORDER BY root_id",
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    let after_ids: Vec<String> = after
        .iter()
        .filter_map(|r| {
            r.get("root_id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect();
    eprintln!("[chained] focus_roots after INSERT: {:?}", after_ids);
    assert_eq!(
        after_ids,
        vec!["b1", "b2"],
        "Matview query should return both self (b1) and child (b2)"
    );

    // The bug assertion
    assert!(
        focus_roots_events > 0,
        "BUG: INSERT INTO block with parent_id matching the focus target should \
         trigger CDC for the chained focus_roots matview, but 0 events were delivered. \
         The matview query correctly returns {:?}, proving IVM updated the data \
         but didn't fire CDC. Simple (non-chained) case works fine.",
        after_ids
    );
}

/// Double-chained case: a watch matview on top of focus_roots (matview-on-matview-on-matview).
/// This matches what `query_and_watch()` does: it creates a matview like
/// `SELECT ... FROM focus_roots fr JOIN block b ON b.id = fr.root_id WHERE fr.region = '...'`
#[tokio::test]
async fn watch_matview_on_focus_roots_cdc_after_insert() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("repro_watch.db");

    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    // Create tables
    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS navigation_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                region TEXT NOT NULL,
                block_id TEXT,
                timestamp TEXT DEFAULT (datetime('now'))
            )",
        )
        .await
        .unwrap();
    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS navigation_cursor (
                region TEXT PRIMARY KEY,
                history_id INTEGER REFERENCES navigation_history(id)
            )",
        )
        .await
        .unwrap();
    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS block (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL DEFAULT '',
                parent_id TEXT NOT NULL DEFAULT ''
            )",
        )
        .await
        .unwrap();

    // Initial data
    handle
        .execute(
            "INSERT INTO block (id, content, parent_id) VALUES ('b1', 'parent', 'doc:root')",
            vec![],
        )
        .await
        .unwrap();
    handle
        .execute(
            "INSERT INTO navigation_history (id, region, block_id) VALUES (1, 'left_sidebar', 'b1')",
            vec![],
        )
        .await
        .unwrap();
    handle
        .execute(
            "INSERT INTO navigation_cursor (region, history_id) VALUES ('left_sidebar', 1)",
            vec![],
        )
        .await
        .unwrap();

    // Create matview chain: current_focus → focus_roots
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW current_focus AS
             SELECT nc.region, nh.block_id, nh.timestamp
             FROM navigation_cursor nc
             JOIN navigation_history nh ON nc.history_id = nh.id",
        )
        .await
        .unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW focus_roots AS
             SELECT cf.region, cf.block_id, b.id AS root_id
             FROM current_focus AS cf
             JOIN block AS b ON b.parent_id = cf.block_id
             UNION ALL
             SELECT cf.region, cf.block_id, b.id AS root_id
             FROM current_focus AS cf
             JOIN block AS b ON b.id = cf.block_id",
        )
        .await
        .unwrap();

    // Create the "watch" matview on top of focus_roots (what query_and_watch creates)
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW mv_region_watch AS
             SELECT fr.root_id AS id, b.content, b.parent_id
             FROM focus_roots fr
             JOIN block b ON b.id = fr.root_id
             WHERE fr.region = 'left_sidebar'",
        )
        .await
        .unwrap();

    // Drain setup events
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    while cdc_rx.try_recv().is_ok() {}

    // Verify initial state
    let initial = handle
        .query(
            "SELECT id FROM mv_region_watch ORDER BY id",
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    let initial_ids: Vec<String> = initial
        .iter()
        .filter_map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect();
    eprintln!("[watch] Initial mv_region_watch: {:?}", initial_ids);
    assert_eq!(initial_ids, vec!["b1"]);

    // INSERT child block
    handle
        .execute(
            "INSERT INTO block (id, content, parent_id) VALUES ('b2', 'child', 'b1')",
            vec![],
        )
        .await
        .unwrap();
    eprintln!("[watch] Inserted child block b2 with parent_id=b1");

    // Wait for CDC
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let mut watch_events = 0;
    let mut focus_roots_events = 0;
    while let Ok(batch) = cdc_rx.try_recv() {
        let name = &batch.metadata.relation_name;
        let count = batch.inner.items.len();
        eprintln!("[watch] CDC: relation='{}' items={}", name, count);
        if name.starts_with("mv_region_watch") {
            watch_events += count;
        } else if name.starts_with("focus_roots") {
            focus_roots_events += count;
        }
    }
    eprintln!(
        "[watch] CDC summary: mv_region_watch={}, focus_roots={}",
        watch_events, focus_roots_events
    );

    // Verify the watch matview query returns both rows
    let after = handle
        .query(
            "SELECT id FROM mv_region_watch ORDER BY id",
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    let after_ids: Vec<String> = after
        .iter()
        .filter_map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect();
    eprintln!("[watch] mv_region_watch after INSERT: {:?}", after_ids);
    assert_eq!(
        after_ids,
        vec!["b1", "b2"],
        "Watch matview should return both b1 and b2"
    );

    assert!(
        watch_events > 0,
        "BUG: INSERT INTO block should propagate through the matview chain \
         (block → focus_roots → mv_region_watch) and trigger CDC for the watch \
         matview. Got 0 events. focus_roots got {} events. \
         The watch query correctly returns {:?}.",
        focus_roots_events,
        after_ids
    );
}

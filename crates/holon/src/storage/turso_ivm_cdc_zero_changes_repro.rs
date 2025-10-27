//! Minimal reproducer: Turso IVM CDC doesn't deliver changes after UPDATE on base table
//!
//! Steps:
//!   1. Create table `block` with columns (id, content, properties)
//!   2. INSERT a row
//!   3. CREATE MATERIALIZED VIEW over `SELECT id, content FROM block`
//!   4. UPDATE the row's `content` column (rows_affected=1)
//!   5. Observe: no CDC event is broadcast for the matview
//!
//! The CDC callback fires for the matview but with changes=0 (empty changeset).
//! Our code skips empty batches, so the broadcast subscriber never learns about
//! the update. This causes the production frontend to show stale data.
//!
//! Run with:
//!   cargo test -p holon turso_ivm_cdc_zero_changes_repro -- --nocapture

use super::turso::TursoBackend;
use tempfile::TempDir;

/// Simple case: INSERT → CREATE MATVIEW → UPDATE → expect CDC
#[tokio::test]
async fn matview_cdc_after_update_simple() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("repro.db");

    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);

    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    // Setup
    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS block (
                id TEXT PRIMARY KEY,
                content TEXT NOT NULL DEFAULT '',
                properties TEXT NOT NULL DEFAULT '{}'
            )",
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO block (id, content) VALUES ('b1', 'original')",
            vec![],
        )
        .await
        .unwrap();

    // Drain setup events
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    while cdc_rx.try_recv().is_ok() {}

    // Create matview
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS mv_repro AS SELECT id, content FROM block",
        )
        .await
        .unwrap();

    // Drain matview creation events
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    while cdc_rx.try_recv().is_ok() {}

    // UPDATE
    let rows = handle
        .execute(
            "UPDATE block SET content = 'updated' WHERE id = 'b1'",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(rows, 1);
    eprintln!("[repro] UPDATE rows_affected=1");

    // Wait for CDC
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Count matview CDC events
    let mut matview_events = 0;
    let mut other_events = 0;
    while let Ok(batch) = cdc_rx.try_recv() {
        if batch.metadata.relation_name.starts_with("mv_repro") {
            matview_events += batch.inner.items.len();
            eprintln!(
                "[repro] CDC: relation='{}' items={}",
                batch.metadata.relation_name,
                batch.inner.items.len()
            );
        } else {
            other_events += 1;
        }
    }

    eprintln!(
        "[repro] matview_events={}, other_events={}",
        matview_events, other_events
    );

    // Verify SQL table has the update
    let result = handle
        .query(
            "SELECT content FROM block WHERE id = 'b1'",
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    assert_eq!(
        result[0].get("content").unwrap().as_string().unwrap(),
        "updated"
    );

    // Verify matview has the update (query should work even if CDC didn't fire)
    let mv = handle
        .query(
            "SELECT content FROM mv_repro WHERE id = 'b1'",
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    let mv_content = mv[0].get("content").unwrap().as_string().unwrap();
    eprintln!("[repro] matview query content='{}'", mv_content);

    assert!(
        matview_events > 0,
        "BUG: UPDATE block SET content='updated' affected 1 row, \
         but matview CDC delivered 0 events. \
         The matview query returns '{}'. \
         Turso IVM fires the CDC callback but with an empty changeset.",
        mv_content
    );
}

/// Variant with more columns (matching production schema more closely)
#[tokio::test]
async fn matview_cdc_after_update_full_schema() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("repro2.db");

    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, mut cdc_rx) = tokio::sync::broadcast::channel(1024);

    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    // Production-like schema
    handle
        .execute_ddl(
            "CREATE TABLE IF NOT EXISTS block (
                id TEXT PRIMARY KEY,
                parent_id TEXT NOT NULL DEFAULT '',
                document_id TEXT NOT NULL DEFAULT '',
                content TEXT NOT NULL DEFAULT '',
                content_type TEXT NOT NULL DEFAULT 'text',
                source_language TEXT,
                source_name TEXT,
                properties TEXT NOT NULL DEFAULT '{}',
                created_at INTEGER NOT NULL DEFAULT 0,
                updated_at INTEGER NOT NULL DEFAULT 0,
                _change_origin TEXT
            )",
        )
        .await
        .unwrap();

    // Insert multiple rows (some matview bugs are row-count dependent)
    for i in 0..5 {
        handle
            .execute(
                &format!(
                    "INSERT INTO block (id, parent_id, document_id, content) \
                     VALUES ('block:b{}', 'doc:root', 'doc:d1', 'content {}')",
                    i, i
                ),
                vec![],
            )
            .await
            .unwrap();
    }

    // Drain
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    while cdc_rx.try_recv().is_ok() {}

    // Create matview selecting a subset of columns (like production watches)
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW IF NOT EXISTS mv_full AS \
             SELECT id, content, content_type, source_language, parent_id FROM block",
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    while cdc_rx.try_recv().is_ok() {}

    // UPDATE one row's content
    let rows = handle
        .execute(
            "UPDATE block SET content = 'CHANGED' WHERE id = 'block:b2'",
            vec![],
        )
        .await
        .unwrap();
    assert_eq!(rows, 1);

    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let mut matview_events = 0;
    while let Ok(batch) = cdc_rx.try_recv() {
        if batch.metadata.relation_name.starts_with("mv_full") {
            matview_events += batch.inner.items.len();
        }
    }

    let mv = handle
        .query(
            "SELECT content FROM mv_full WHERE id = 'block:b2'",
            std::collections::HashMap::new(),
        )
        .await
        .unwrap();
    let mv_content = mv[0].get("content").unwrap().as_string().unwrap();

    eprintln!(
        "[repro:full] matview_events={}, matview content='{}'",
        matview_events, mv_content
    );

    assert!(
        matview_events > 0,
        "BUG: UPDATE affected 1 row but matview CDC delivered 0 events. \
         Matview query returns '{}' (expected 'CHANGED').",
        mv_content
    );
}

//! Increment-0 (tabs) engine split: `focus_roots` holds ALL open tabs for a
//! region, but the cursor-joined main-panel query renders exactly the ACTIVE
//! tab. `activate` (moving `navigation_cursor`) flips which tab renders WITHOUT
//! reordering or closing the others (Q3 stable order). Without the cursor
//! filter, multi-open main would stack every open page in the panel.
//!
//! Run with:
//!   cargo test -p holon tabs_cursor_filtered_panel -- --nocapture

use std::collections::BTreeSet;
use std::collections::HashMap;

use holon::storage::turso::TursoBackend;
use tempfile::TempDir;

fn ids(rows: &[holon_api::StorageEntity], col: &str) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| {
            r.get(col)
                .and_then(|v| v.as_string())
                .map(|s| s.to_string())
        })
        .collect()
}

#[tokio::test]
async fn cursor_filter_renders_only_active_tab() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("tabs.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    handle
        .execute_ddl(
            "CREATE TABLE navigation_history (id INTEGER PRIMARY KEY AUTOINCREMENT, region TEXT \
             NOT NULL, block_id TEXT, timestamp TEXT DEFAULT (datetime('now')), closed_at TEXT \
             NULL)",
        )
        .await
        .unwrap();
    handle
        .execute_ddl("CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER)")
        .await
        .unwrap();
    handle
        .execute_ddl(
            "CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '', parent_id \
                      TEXT DEFAULT '')",
        )
        .await
        .unwrap();

    // Real `focus_roots` matview (open rows per region; multi-tab capable —
    // mirrors crates/holon-turso/sql/schema/matview_focus_roots.sql).
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW focus_roots AS SELECT region, block_id AS root_id, timestamp \
             AS added_ts, id AS history_id FROM navigation_history WHERE closed_at IS NULL AND \
             block_id IS NOT NULL",
        )
        .await
        .unwrap();
    // Increment-0 cursor-filtered main panel (simplified, non-recursive: the
    // load-bearing part is the `navigation_cursor` equi-join added in
    // assets/default/index.org).
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW main_panel AS SELECT b.id AS id, b.content AS content FROM \
             focus_roots fr JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = \
             fr.history_id JOIN block b ON b.id = fr.root_id WHERE fr.region = 'main'",
        )
        .await
        .unwrap();

    handle
        .execute(
            "INSERT INTO block (id, content) VALUES ('doc_a', 'A')",
            vec![],
        )
        .await
        .unwrap();
    handle
        .execute(
            "INSERT INTO block (id, content) VALUES ('doc_b', 'B')",
            vec![],
        )
        .await
        .unwrap();
    // Two OPEN tabs — `open_tab` inserts without closing the prior open row.
    handle
        .execute(
            "INSERT INTO navigation_history (id, region, block_id) VALUES (1, 'main', 'doc_a')",
            vec![],
        )
        .await
        .unwrap();
    handle
        .execute(
            "INSERT INTO navigation_history (id, region, block_id) VALUES (2, 'main', 'doc_b')",
            vec![],
        )
        .await
        .unwrap();

    // focus_roots holds BOTH open tabs (the strip renders all of these).
    let roots = ids(
        &handle
            .query(
                "SELECT root_id FROM focus_roots WHERE region='main'",
                HashMap::new(),
            )
            .await
            .unwrap(),
        "root_id",
    );
    assert_eq!(
        roots,
        BTreeSet::from(["doc_a".to_string(), "doc_b".to_string()]),
        "focus_roots must hold BOTH open tabs"
    );

    // Cursor on tab 1 → panel renders ONLY doc_a (not both).
    handle
        .execute(
            "INSERT OR REPLACE INTO navigation_cursor (region, history_id) VALUES ('main', 1)",
            vec![],
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let panel = ids(
        &handle
            .query("SELECT id FROM main_panel", HashMap::new())
            .await
            .unwrap(),
        "id",
    );
    assert_eq!(
        panel,
        BTreeSet::from(["doc_a".to_string()]),
        "active tab = doc_a; panel must show only doc_a, never both open tabs"
    );

    // Activate tab 2 (move the cursor) → panel flips to ONLY doc_b.
    handle
        .execute(
            "UPDATE navigation_cursor SET history_id = 2 WHERE region = 'main'",
            vec![],
        )
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let panel = ids(
        &handle
            .query("SELECT id FROM main_panel", HashMap::new())
            .await
            .unwrap(),
        "id",
    );
    assert_eq!(
        panel,
        BTreeSet::from(["doc_b".to_string()]),
        "after activate, panel must show only doc_b"
    );

    // Tabs unchanged: activate reorders/closes nothing (Q3 stable order).
    let roots_after = ids(
        &handle
            .query(
                "SELECT root_id FROM focus_roots WHERE region='main'",
                HashMap::new(),
            )
            .await
            .unwrap(),
        "root_id",
    );
    assert_eq!(
        roots_after,
        BTreeSet::from(["doc_a".to_string(), "doc_b".to_string()]),
        "activate must not close or reorder the open tabs"
    );
}

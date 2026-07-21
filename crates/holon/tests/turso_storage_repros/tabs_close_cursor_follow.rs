//! Increment-4 (tabs) close semantics: closing the ACTIVE main tab must move
//! the cursor to a neighbor so the cursor-joined main panel keeps rendering a
//! real tab instead of going blank. `close(history_id)` soft-closes the row;
//! when that row was the region's `navigation_cursor` target, the cursor
//! follows to the nearest still-open tab — LEFT neighbor preferred (the tab
//! before it in stable insertion order), then RIGHT, and when no tab is left
//! the cursor row is dropped so the panel falls through to its default render.
//!
//! Drives the REAL `NavigationProvider` through its public
//! `OperationProvider::execute_operation` dispatch (open_tab / activate /
//! close) so the include_str! SQL and the cursor-follow branch are exercised
//! exactly as production does.
//!
//! Also covers Increment-3 persistence: the open set + cursor live in ordinary
//! tables, so reopening the same db file preserves them (boot restores tabs).
//!
//! Run with:
//!   cargo test -p holon --features test-helpers --test turso_storage_repros \
//!     tabs_close_cursor_follow -- --nocapture

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::sync::Arc;

use holon::navigation::NavigationProvider;
use holon::storage::DbHandle;
use holon::storage::turso::TursoBackend;
use holon_api::EntityName;
use holon_api::Region;
use holon_api::Value;
use holon_core::OperationProvider;
use tempfile::TempDir;

fn ids(rows: &[holon_api::StorageEntity], col: &str) -> BTreeSet<String> {
    rows.iter()
        .filter_map(|r| r.get(col).and_then(|v| v.as_string()).map(str::to_string))
        .collect()
}

async fn panel_ids(handle: &DbHandle) -> BTreeSet<String> {
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    ids(
        &handle
            .query("SELECT id FROM main_panel", HashMap::new())
            .await
            .unwrap(),
        "id",
    )
}

async fn cursor_history_id(handle: &DbHandle) -> Option<i64> {
    handle
        .query(
            "SELECT history_id FROM navigation_cursor WHERE region = 'main'",
            HashMap::new(),
        )
        .await
        .unwrap()
        .first()
        .and_then(|r| r.get("history_id"))
        .and_then(|v| v.as_i64())
}

fn param(pairs: &[(&str, Value)]) -> holon_api::StorageEntity {
    pairs
        .iter()
        .map(|(k, v)| (Arc::from(*k), v.clone()))
        .collect()
}

async fn setup(handle: &DbHandle) {
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
        .execute_ddl("CREATE TABLE block (id TEXT PRIMARY KEY, content TEXT DEFAULT '')")
        .await
        .unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW focus_roots AS SELECT region, block_id AS root_id, timestamp \
             AS added_ts, id AS history_id FROM navigation_history WHERE closed_at IS NULL AND \
             block_id IS NOT NULL",
        )
        .await
        .unwrap();
    handle
        .execute_ddl(
            "CREATE MATERIALIZED VIEW main_panel AS SELECT b.id AS id, b.content AS content FROM \
             focus_roots fr JOIN navigation_cursor nc ON nc.region = fr.region AND nc.history_id = \
             fr.history_id JOIN block b ON b.id = fr.root_id WHERE fr.region = 'main'",
        )
        .await
        .unwrap();
    for id in ["doc_a", "doc_b", "doc_c"] {
        handle
            .execute(
                &format!("INSERT INTO block (id, content) VALUES ('{id}', '{id}')"),
                vec![],
            )
            .await
            .unwrap();
    }
}

async fn open_tab(provider: &NavigationProvider, block_id: &str) {
    provider
        .execute_operation(
            &EntityName::new("navigation"),
            "open_tab",
            param(&[
                ("region", Value::from(Region::Main)),
                ("block_id", Value::String(block_id.to_string())),
            ]),
        )
        .await
        .unwrap();
}

async fn activate(provider: &NavigationProvider, history_id: i64) {
    provider
        .execute_operation(
            &EntityName::new("navigation"),
            "activate",
            param(&[
                ("region", Value::from(Region::Main)),
                ("history_id", Value::Integer(history_id)),
            ]),
        )
        .await
        .unwrap();
}

async fn close(provider: &NavigationProvider, history_id: i64) {
    provider
        .execute_operation(
            &EntityName::new("navigation"),
            "close",
            param(&[("history_id", Value::Integer(history_id))]),
        )
        .await
        .unwrap();
}

/// Closing the active tab follows the cursor to a neighbor (left, then right),
/// and dropping the last tab clears the cursor so the panel goes empty.
#[tokio::test]
async fn close_active_tab_follows_cursor_to_neighbor() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("tabs_close.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    setup(&handle).await;
    let provider = NavigationProvider::new(handle.clone());

    // Three open tabs — ids 1,2,3; open_tab leaves the cursor on the last one.
    open_tab(&provider, "doc_a").await;
    open_tab(&provider, "doc_b").await;
    open_tab(&provider, "doc_c").await;
    assert_eq!(cursor_history_id(&handle).await, Some(3));
    assert_eq!(panel_ids(&handle).await, BTreeSet::from(["doc_c".into()]));

    // Activate the middle tab, then close it: cursor follows LEFT to doc_a (1).
    activate(&provider, 2).await;
    assert_eq!(panel_ids(&handle).await, BTreeSet::from(["doc_b".into()]));
    close(&provider, 2).await;
    assert_eq!(
        cursor_history_id(&handle).await,
        Some(1),
        "closing the active middle tab must move the cursor to the LEFT neighbor (doc_a)"
    );
    assert_eq!(
        panel_ids(&handle).await,
        BTreeSet::from(["doc_a".into()]),
        "panel must render the left-neighbor tab, never go blank"
    );

    // Close the now-active leftmost tab (1): no left neighbor -> follow RIGHT to
    // doc_c (3), the only remaining open tab.
    close(&provider, 1).await;
    assert_eq!(
        cursor_history_id(&handle).await,
        Some(3),
        "closing the leftmost active tab must fall back to the RIGHT neighbor (doc_c)"
    );
    assert_eq!(panel_ids(&handle).await, BTreeSet::from(["doc_c".into()]));

    // Close the last remaining tab (3): no neighbor -> cursor cleared, panel empty.
    close(&provider, 3).await;
    assert_eq!(
        cursor_history_id(&handle).await,
        None,
        "closing the last open tab must clear the cursor"
    );
    assert_eq!(
        panel_ids(&handle).await,
        BTreeSet::new(),
        "with no open tab the cursor-joined panel renders nothing"
    );
}

/// Closing a NON-active tab must leave the active tab (cursor) untouched.
#[tokio::test]
async fn close_inactive_tab_leaves_cursor() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("tabs_close_inactive.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    setup(&handle).await;
    let provider = NavigationProvider::new(handle.clone());

    open_tab(&provider, "doc_a").await; // 1
    open_tab(&provider, "doc_b").await; // 2 (cursor)
    assert_eq!(cursor_history_id(&handle).await, Some(2));

    // Close the inactive tab 1 -> cursor stays on 2, panel still doc_b.
    close(&provider, 1).await;
    assert_eq!(cursor_history_id(&handle).await, Some(2));
    assert_eq!(panel_ids(&handle).await, BTreeSet::from(["doc_b".into()]));
}

/// Increment-3: the open set + cursor are ordinary rows, so reopening the same
/// db file preserves them — boot restores the persisted tabs and active tab.
#[tokio::test]
async fn open_set_and_cursor_persist_across_reopen() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("tabs_persist.db");

    {
        let db = TursoBackend::open_database(&db_path).expect("open db");
        let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
        let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
        setup(&handle).await;
        let provider = NavigationProvider::new(handle.clone());
        open_tab(&provider, "doc_a").await; // 1
        open_tab(&provider, "doc_b").await; // 2
        activate(&provider, 1).await; // active tab = doc_a
        assert_eq!(cursor_history_id(&handle).await, Some(1));
    }

    // Reopen the same file — the base tables (and matviews) persist.
    let db = TursoBackend::open_database(&db_path).expect("reopen db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("recreate backend");

    let open_roots = ids(
        &handle
            .query(
                "SELECT block_id AS root_id FROM navigation_history WHERE region='main' AND \
                 closed_at IS NULL AND block_id IS NOT NULL",
                HashMap::new(),
            )
            .await
            .unwrap(),
        "root_id",
    );
    assert_eq!(
        open_roots,
        BTreeSet::from(["doc_a".into(), "doc_b".into()]),
        "both open tabs must survive a reopen"
    );
    assert_eq!(
        cursor_history_id(&handle).await,
        Some(1),
        "the active tab (cursor) must survive a reopen"
    );
}

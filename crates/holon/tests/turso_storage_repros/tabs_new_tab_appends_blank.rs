//! `navigation.new_tab(region)` opens one more tab on the region's home and
//! moves the cursor to it — the browser's new-tab button.
//!
//! A new tab is BLANK: it inserts an open `navigation_history` row with a NULL
//! `block_id` (the same shape `go_home` records), so the cursor-joined main
//! panel falls through to its default render rather than showing the page the
//! user was on. Unlike `go_home` it closes nothing, and unlike `open_tab` it
//! names no target, so pressing it twice opens two tabs.
//!
//! Drives the REAL `NavigationProvider` through `execute_operation`, the same
//! dispatch production uses.
//!
//! Run with:
//!   cargo test -p holon --features test-helpers --test turso_storage_repros \
//!     tabs_new_tab_appends_blank -- --nocapture

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
    for id in ["doc_a", "doc_b"] {
        handle
            .execute(
                &format!("INSERT INTO block (id, content) VALUES ('{id}', '{id}')"),
                vec![],
            )
            .await
            .unwrap();
    }
}

async fn open_tab(provider: &NavigationProvider, region: Region, block_id: &str) {
    provider
        .execute_operation(
            &EntityName::new("navigation"),
            "open_tab",
            param(&[
                ("region", Value::from(region)),
                ("block_id", Value::String(block_id.to_string())),
            ]),
        )
        .await
        .unwrap();
}

async fn new_tab(
    provider: &NavigationProvider,
    region: Region,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    provider
        .execute_operation(
            &EntityName::new("navigation"),
            "new_tab",
            param(&[("region", Value::from(region))]),
        )
        .await
        .map(|_| ())
}

/// `(id, block_id)` of every OPEN row in a region, in insertion order — the
/// tabs the chrome counts and lists.
async fn open_rows(handle: &DbHandle, region: &str) -> Vec<(i64, Option<String>)> {
    handle
        .query(
            &format!(
                "SELECT id, block_id FROM navigation_history WHERE region = '{region}' AND \
                 closed_at IS NULL ORDER BY id"
            ),
            HashMap::new(),
        )
        .await
        .unwrap()
        .iter()
        .map(|r| {
            (
                r.get("id").and_then(|v| v.as_i64()).expect("id is an int"),
                r.get("block_id")
                    .and_then(|v| v.as_string())
                    .map(str::to_string),
            )
        })
        .collect()
}

async fn cursor_history_id(handle: &DbHandle, region: &str) -> Option<i64> {
    handle
        .query(
            &format!("SELECT history_id FROM navigation_cursor WHERE region = '{region}'"),
            HashMap::new(),
        )
        .await
        .unwrap()
        .first()
        .and_then(|r| r.get("history_id"))
        .and_then(|v| v.as_i64())
}

#[tokio::test]
async fn new_tab_appends_a_blank_tab_and_moves_the_cursor_to_it() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("tabs_new.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    setup(&handle).await;
    let provider = NavigationProvider::new(handle.clone());

    open_tab(&provider, Region::Main, "doc_a").await;
    open_tab(&provider, Region::Main, "doc_b").await;
    let before = open_rows(&handle, "main").await;
    assert_eq!(
        before.len(),
        2,
        "two tabs must be open before the claim, else appending one proves nothing: {before:?}"
    );

    new_tab(&provider, Region::Main)
        .await
        .expect("navigation.new_tab is a registered operation");

    let after = open_rows(&handle, "main").await;
    assert_eq!(
        after.len(),
        before.len() + 1,
        "new_tab must APPEND a tab, closing none: was {before:?}, now {after:?}"
    );
    let (new_id, new_block) = after.last().cloned().expect("the appended row");
    assert_eq!(
        new_block, None,
        "a new tab is blank — it records a NULL block_id like go_home, so the main panel falls \
         through to its default render instead of repeating the page the user was on; it holds \
         {new_block:?}"
    );
    assert_eq!(
        cursor_history_id(&handle, "main").await,
        Some(new_id),
        "the cursor must move to the tab that was just created — a new tab the user cannot see is \
         not a new tab"
    );
    assert_eq!(
        before.iter().map(|(id, _)| *id).collect::<Vec<_>>(),
        after
            .iter()
            .take(before.len())
            .map(|(id, _)| *id)
            .collect::<Vec<_>>(),
        "the tabs that were already open must keep their identity and their order"
    );

    // Twice pressed, twice opened: new_tab names no target, so it has no
    // `(region, block_id)` key to dedup on the way open_tab does.
    new_tab(&provider, Region::Main)
        .await
        .expect("new_tab a second time");
    let twice = open_rows(&handle, "main").await;
    assert_eq!(
        twice.len(),
        after.len() + 1,
        "pressing new tab twice must open two tabs: {twice:?}"
    );
    let blanks: Vec<i64> = twice
        .iter()
        .filter(|(_, block)| block.is_none())
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(
        blanks.len(),
        2,
        "two presses must leave TWO blank tabs standing, not one row reused: open rows are \
         {twice:?}"
    );
    let latest = blanks.last().copied().expect("the second blank tab");
    assert_eq!(
        cursor_history_id(&handle, "main").await,
        Some(latest),
        "the cursor must sit on the tab the SECOND press created ({latest}); silently activating \
         the blank tab that already existed would make the button look broken"
    );
}

/// `new_tab` is region-scoped: it touches neither another region's open rows
/// nor its cursor.
#[tokio::test]
async fn new_tab_leaves_other_regions_alone() {
    let temp = TempDir::new().unwrap();
    let db_path = temp.path().join("tabs_new_region.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = tokio::sync::broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");
    setup(&handle).await;
    let provider = NavigationProvider::new(handle.clone());

    open_tab(&provider, Region::Main, "doc_a").await;
    open_tab(&provider, Region::RightSidebar, "doc_b").await;
    let sidebar_before = open_rows(&handle, "right_sidebar").await;
    let sidebar_cursor_before = cursor_history_id(&handle, "right_sidebar").await;
    assert!(
        !sidebar_before.is_empty(),
        "the sidebar must hold an open row before the claim, else it is vacuous"
    );

    new_tab(&provider, Region::Main)
        .await
        .expect("navigation.new_tab is a registered operation");

    assert_eq!(
        open_rows(&handle, "right_sidebar").await,
        sidebar_before,
        "a new MAIN tab must leave the sidebar's open rows untouched"
    );
    assert_eq!(
        cursor_history_id(&handle, "right_sidebar").await,
        sidebar_cursor_before,
        "a new MAIN tab must leave the sidebar's cursor untouched"
    );
}

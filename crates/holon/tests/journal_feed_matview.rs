//! View-level test for the journal-feed chained matview
//! (`docs/Plans/JournalFeed-2026-07-18.md`).
//!
//! Boots the production chain — `block` matview → `journal_day_pages`
//! (day-page detection) → `journal_feed` — over real `block_raw` / `block_tags`
//! tables, then asserts:
//!   1. both new matviews are visible in `sqlite_master` (MCP `list_tables`)
//!      and the chain builds WITHOUT hanging (the whole point of the ruling:
//!      chained matviews are reliable on this Turso rev);
//!   2. `journal_feed` contains exactly the `Page`-tagged children of
//!      `block:journals`, each carrying `expand_default = 1`, and the read
//!      orders them newest-first;
//!   3. IVM maintains the chain O(delta): creating a NEW day page (block_raw +
//!      block_tags rows) makes it appear in `journal_feed` with no rebuild.

use std::collections::HashMap;

use holon::storage::schema_module::SchemaModule;
use holon::storage::turso::TursoBackend;
use holon_turso::schema_modules::BlockMatviewSchemaModule;
use holon_turso::schema_modules::BlockSchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::JournalDayPagesSchemaModule;
use holon_turso::schema_modules::JournalFeedSchemaModule;
use tempfile::TempDir;
use tokio::sync::broadcast;

type Handle = holon::storage::turso::DbHandle;

async fn insert_block(handle: &Handle, id: &str, parent_id: &str, content: &str) {
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, content) VALUES (?, ?, ?)",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(parent_id.into()),
                turso::Value::Text(content.into()),
            ],
        )
        .await
        .expect("block_raw insert");
}

async fn tag_page(handle: &Handle, id: &str) {
    handle
        .execute(
            "INSERT INTO block_tags (block_id, tag) VALUES (?, 'Page')",
            vec![turso::Value::Text(id.into())],
        )
        .await
        .expect("block_tags insert");
}

async fn feed_contents(handle: &Handle) -> Vec<(String, i64)> {
    let rows = handle
        .query(
            "SELECT content, expand_default FROM journal_feed ORDER BY content DESC",
            HashMap::new(),
        )
        .await
        .expect("journal_feed query");
    rows.iter()
        .map(|r| {
            let content = r.get("content").unwrap().as_string().unwrap().to_string();
            let expand = match r.get("expand_default") {
                Some(holon_api::Value::Integer(i)) => *i,
                other => panic!("expand_default not an integer: {other:?}"),
            };
            (content, expand)
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn journal_feed_chain_lists_day_pages_and_maintains_deltas() {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("journal_feed.db");
    let db = TursoBackend::open_database(&db_path).expect("open db");
    let (cdc_tx, _cdc_rx) = broadcast::channel(1024);
    let (_backend, handle) = TursoBackend::new(db, cdc_tx).expect("create backend");

    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("FKs");

    // Production chain: block_raw + junctions → block matview → detection → feed.
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("CoreSchemaModule");
    BlockSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("BlockSchemaModule");
    BlockMatviewSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("BlockMatviewSchemaModule");
    JournalDayPagesSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("journal_day_pages matview (chained on block matview) must build");
    JournalFeedSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("journal_feed matview (matview-on-matview) must build — no hang");

    // (1) Both matviews visible in sqlite_master (what MCP list_tables reads).
    let listed = handle
        .query(
            "SELECT name FROM sqlite_master WHERE name IN ('journal_day_pages', 'journal_feed') \
             ORDER BY name",
            HashMap::new(),
        )
        .await
        .expect("sqlite_master query");
    let names: Vec<String> = listed
        .iter()
        .map(|r| r.get("name").unwrap().as_string().unwrap().to_string())
        .collect();
    assert_eq!(
        names,
        vec!["journal_day_pages".to_string(), "journal_feed".to_string()],
        "both chain matviews must be visible in list_tables (sqlite_master)"
    );

    // Seed the journals page + two day pages (Page-tagged) + one NON-page child.
    insert_block(&handle, "block:journals", "sentinel:no_parent", "Journals").await;
    insert_block(&handle, "block:2026-07-16", "block:journals", "2026-07-16").await;
    tag_page(&handle, "block:2026-07-16").await;
    insert_block(&handle, "block:2026-07-17", "block:journals", "2026-07-17").await;
    tag_page(&handle, "block:2026-07-17").await;
    // A non-Page child of journals must NOT surface in the feed.
    insert_block(&handle, "block:stray", "block:journals", "not a day page").await;

    // (2) Feed = only the Page-tagged day pages, newest-first, expand_default=1.
    assert_eq!(
        feed_contents(&handle).await,
        vec![
            ("2026-07-17".to_string(), 1),
            ("2026-07-16".to_string(), 1),
        ],
        "journal_feed lists only Page-tagged day pages, newest-first, expanded"
    );

    // (3) O(delta) maintenance: a NEW day page flows through the whole chain.
    insert_block(&handle, "block:2026-07-18", "block:journals", "2026-07-18").await;
    tag_page(&handle, "block:2026-07-18").await;
    assert_eq!(
        feed_contents(&handle).await,
        vec![
            ("2026-07-18".to_string(), 1),
            ("2026-07-17".to_string(), 1),
            ("2026-07-16".to_string(), 1),
        ],
        "new day page appears in journal_feed via incremental chain maintenance"
    );
}

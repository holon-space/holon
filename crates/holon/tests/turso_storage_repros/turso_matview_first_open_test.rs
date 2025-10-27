//! Regression test for the matview-first-read partial-result bug.
//!
//! Originally observed in holon production: the first
//! `mcp__holon-mcp__execute_*` against the `block` matview returned 0 (or a
//! small partial subset) of rows, with subsequent calls returning the real
//! result. Fixed upstream in nightscape@holon `290fbb4ff` (*"fix: IVM
//! matview cursor returns partial result on first read after IO yield"*).
//!
//! This is the holon-stack regression gate. Pure-Turso variant lives at
//! `bigdata/turso/bindings/rust/tests/matview_first_open.rs`.

use holon::storage::DbHandle;
use holon::storage::test_helpers::create_test_backend_at_path;
use holon_api::Value;
use std::collections::HashMap;
use tempfile::TempDir;

const NUM_ROWS: usize = 1000;

const MATVIEW_DDL: &str = "CREATE MATERIALIZED VIEW IF NOT EXISTS block AS
    SELECT
        b.id, b.parent_id, b.depth, b.sort_key, b.content, b.content_type,
        b.source_language, b.source_name, b.properties, b.marks, b.collapsed,
        b.completed, b.block_type, b.created_at, b.updated_at, b._change_origin,
        COALESCE(json_group_array(bt.tag)         FILTER (WHERE bt.tag         IS NOT NULL), '[]') AS tags,
        COALESCE(json_group_array(br.required_id) FILTER (WHERE br.required_id IS NOT NULL), '[]') AS requires
    FROM block_raw b
    LEFT OUTER JOIN block_tags     bt ON bt.block_id = b.id
    LEFT OUTER JOIN block_requires br ON br.block_id = b.id
    GROUP BY
        b.id, b.parent_id, b.depth, b.sort_key, b.content, b.content_type,
        b.source_language, b.source_name, b.properties, b.marks, b.collapsed,
        b.completed, b.block_type, b.created_at, b.updated_at, b._change_origin";

#[tokio::test]
async fn matview_first_read_full_result_matview_before_inserts() {
    assert_consistent_first_read(true).await;
}

#[tokio::test]
async fn matview_first_read_full_result_matview_after_inserts() {
    assert_consistent_first_read(false).await;
}

async fn assert_consistent_first_read(matview_before_inserts: bool) {
    let temp_dir = TempDir::new().unwrap();
    let db_path = temp_dir.path().join("matview_first_open.db");

    let backend = create_test_backend_at_path(&db_path).await;
    let db = backend.handle();

    db.execute(
        "CREATE TABLE IF NOT EXISTS block_raw (
            id TEXT PRIMARY KEY,
            parent_id TEXT,
            depth INTEGER NOT NULL DEFAULT 0,
            sort_key TEXT NOT NULL DEFAULT 'A0',
            content TEXT NOT NULL DEFAULT '',
            content_type TEXT NOT NULL DEFAULT 'text',
            source_language TEXT,
            source_name TEXT,
            properties TEXT,
            marks TEXT,
            collapsed INTEGER NOT NULL DEFAULT 0,
            completed INTEGER NOT NULL DEFAULT 0,
            block_type TEXT NOT NULL DEFAULT 'text',
            created_at INTEGER NOT NULL DEFAULT 0,
            updated_at INTEGER NOT NULL DEFAULT 0,
            _change_origin TEXT
        )",
        vec![],
    )
    .await
    .unwrap();
    db.execute(
        "CREATE TABLE IF NOT EXISTS block_tags (
            block_id TEXT NOT NULL,
            tag TEXT NOT NULL,
            PRIMARY KEY (block_id, tag)
        )",
        vec![],
    )
    .await
    .unwrap();
    db.execute(
        "CREATE TABLE IF NOT EXISTS block_requires (
            block_id TEXT NOT NULL,
            required_id TEXT NOT NULL,
            PRIMARY KEY (block_id, required_id)
        )",
        vec![],
    )
    .await
    .unwrap();

    if matview_before_inserts {
        db.execute(MATVIEW_DDL, vec![]).await.unwrap();
    }

    for i in 0..NUM_ROWS {
        let id = format!("block:row-{:05}", i);
        let parent_id = if i == 0 {
            "doc:root".to_string()
        } else {
            format!("block:row-{:05}", i / 10)
        };
        let props = format!(
            r#"{{"task_state":"{}","gate":"{}"}}"#,
            if i % 7 == 0 { "TODO" } else { "DONE" },
            if i % 5 == 0 { "G1" } else { "G0" },
        );
        db.execute(
            "INSERT INTO block_raw (id, parent_id, content, properties) \
             VALUES (?, ?, ?, ?)",
            vec![
                id.clone().into(),
                parent_id.into(),
                format!("C{}", i).into(),
                props.into(),
            ],
        )
        .await
        .unwrap();
        if i % 4 == 0 {
            db.execute(
                "INSERT OR IGNORE INTO block_tags (block_id, tag) \
                 VALUES (?, ?)",
                vec![id.into(), "agent".to_string().into()],
            )
            .await
            .unwrap();
        }
    }

    if !matview_before_inserts {
        db.execute(MATVIEW_DDL, vec![]).await.unwrap();
    }

    let base = single_count(&db, "SELECT COUNT(*) FROM block_raw").await;
    let first = single_count(&db, "SELECT COUNT(*) FROM block").await;
    let second = single_count(&db, "SELECT COUNT(*) FROM block").await;

    assert_eq!(base, NUM_ROWS as i64, "base table population");
    assert_eq!(
        first, base,
        "first matview read should already see every row \
         (matview_before_inserts={matview_before_inserts})"
    );
    assert_eq!(first, second, "two identical SELECTs must agree");
}

async fn single_count(db: &DbHandle, sql: &str) -> i64 {
    let rows = db.query(sql, HashMap::new()).await.unwrap();
    let row = rows
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("`{sql}` returned no rows"));
    let v = row
        .into_iter()
        .next()
        .unwrap_or_else(|| panic!("`{sql}` row had no columns"))
        .1;
    match v {
        Value::Integer(n) => n,
        other => panic!("`{sql}` expected Integer, got {other:?}"),
    }
}

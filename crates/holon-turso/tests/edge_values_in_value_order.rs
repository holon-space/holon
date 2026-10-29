//! Edge sets read the same through the `block` view and through the hydrated
//! direct read: both list targets in value order, whatever order they were
//! inserted in.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::block_table_names::HYDRATED_BLOCK_COLUMNS;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

use crate::warm_reboot_keeps_matviews::ensure_all;

const EDGES: &[(&str, &str, &str)] = &[
    ("block_tags", "tag", "zebra"),
    ("block_tags", "tag", "apple"),
    ("block_tags", "tag", "mango"),
    ("block_requires", "required_id", "block:z"),
    ("block_requires", "required_id", "block:a"),
    ("block_requires", "required_id", "block:m"),
    ("block_contributes_to", "target_id", "block:z"),
    ("block_contributes_to", "target_id", "block:a"),
];

async fn edges(handle: &DbHandle, sql: &str) -> Vec<(String, String, String)> {
    handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .into_iter()
        .map(|row| {
            let text = |c: &str| match row.get(c) {
                Some(Value::String(s)) => s.clone(),
                other => panic!("{c} is not text: {other:?}"),
            };
            (text("tags"), text("requires"), text("contributes_to"))
        })
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn edge_targets_read_in_value_order_not_insertion_order() {
    let db = TursoBackend::open_database(":memory:").expect("open");
    let (_backend, handle) = TursoBackend::new(db, broadcast::channel(64).0).expect("backend");
    ensure_all(&handle).await;

    for id in ["block:src", "block:z", "block:a", "block:m"] {
        handle
            .execute(
                "INSERT INTO block_raw (id, parent_id, content) VALUES (?, 'sentinel:no_parent', 'x')",
                vec![turso::Value::Text(id.to_string())],
            )
            .await
            .expect("insert block");
    }
    for (table, column, target) in EDGES {
        handle
            .execute(
                &format!("INSERT INTO {table} (block_id, {column}) VALUES ('block:src', ?)"),
                vec![turso::Value::Text(target.to_string())],
            )
            .await
            .unwrap_or_else(|e| panic!("insert into {table}: {e}"));
    }

    let expected = vec![(
        r#"["apple","mango","zebra"]"#.to_string(),
        r#"["block:a","block:m","block:z"]"#.to_string(),
        r#"["block:a","block:z"]"#.to_string(),
    )];
    assert_eq!(
        edges(
            &handle,
            "SELECT tags, requires, contributes_to FROM block WHERE id = 'block:src'"
        )
        .await,
        expected,
        "the block view"
    );
    assert_eq!(
        edges(
            &handle,
            &format!("SELECT {HYDRATED_BLOCK_COLUMNS} FROM block_raw b WHERE b.id = 'block:src'")
        )
        .await,
        expected,
        "the hydrated direct read"
    );
}

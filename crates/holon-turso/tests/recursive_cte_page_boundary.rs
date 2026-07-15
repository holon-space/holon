//! IVM verification: the Phase 3 holon_sql recursive CTE that stops at non-root
//! page boundaries can be created as a Turso matview and produces correct
//! results through inserts and tag mutations.
//!
//! The tested SQL is the main-panel query shape from `assets/default/index.org`.

use std::collections::HashMap;

use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// The main-panel recursive CTE, inlined. The `block` matview is emulated by a
/// plain `block` table (no edge-field hydrations needed for this test). ORDER BY
/// is omitted because Turso IVM rejects column-reference ORDER BY in matview
/// definitions; the production path strips it via `strip_order_by` before DDL.
const MAIN_PANEL_CTE: &str = "\
WITH RECURSIVE focus_descendants AS (\
  SELECT b.*, 0 AS _depth \
  FROM focus_roots fr \
  JOIN block b ON b.id = fr.root_id \
  WHERE fr.region = 'main' \
  UNION ALL \
  SELECT child.*, fd._depth + 1 \
  FROM focus_descendants fd \
  JOIN block child ON child.parent_id = fd.id \
  LEFT JOIN block_tags bt ON bt.block_id = fd.id AND bt.tag = 'Page' \
  WHERE fd._depth = 0 OR bt.block_id IS NULL \
) \
SELECT * FROM focus_descendants";

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    handle
        .execute_ddl(
            "CREATE TABLE block (\
             id TEXT PRIMARY KEY, \
             parent_id TEXT, \
             sort_key TEXT NOT NULL DEFAULT 'A0', \
             content TEXT NOT NULL DEFAULT '', \
             content_type TEXT NOT NULL DEFAULT 'text'\
             )",
        )
        .await
        .expect("create block");
    handle
        .execute_ddl(
            "CREATE TABLE block_tags (\
             block_id TEXT NOT NULL, \
             tag TEXT NOT NULL, \
             PRIMARY KEY (block_id, tag)\
             )",
        )
        .await
        .expect("create block_tags");
    handle
        .execute_ddl(
            "CREATE TABLE focus_roots (\
             region TEXT NOT NULL, \
             root_id TEXT NOT NULL, \
             added_ts INTEGER NOT NULL DEFAULT 0, \
             history_id TEXT\
             )",
        )
        .await
        .expect("create focus_roots");
    handle
}

async fn insert_block(handle: &DbHandle, id: &str, parent_id: &str, content: &str, sort_key: &str) {
    handle
        .execute(
            "INSERT INTO block (id, parent_id, content, sort_key) VALUES (?, ?, ?, ?)",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(parent_id.into()),
                turso::Value::Text(content.into()),
                turso::Value::Text(sort_key.into()),
            ],
        )
        .await
        .expect("insert block");
}

async fn tag_page(handle: &DbHandle, block_id: &str) {
    handle
        .execute(
            "INSERT INTO block_tags (block_id, tag) VALUES (?, 'Page')",
            vec![turso::Value::Text(block_id.into())],
        )
        .await
        .expect("tag page");
}

async fn set_focus(handle: &DbHandle, region: &str, root_id: &str) {
    handle
        .execute(
            "DELETE FROM focus_roots WHERE region = ?",
            vec![turso::Value::Text(region.into())],
        )
        .await
        .expect("clear focus");
    handle
        .execute(
            "INSERT INTO focus_roots (region, root_id) VALUES (?, ?)",
            vec![
                turso::Value::Text(region.into()),
                turso::Value::Text(root_id.into()),
            ],
        )
        .await
        .expect("set focus");
}

async fn read_descendants(handle: &DbHandle) -> Vec<String> {
    let rows = handle
        .query("SELECT id FROM focus_descendants_view ORDER BY _depth, sort_key", HashMap::new())
        .await
        .expect("query view");
    rows.iter()
        .map(|r| match r.get("id") {
            Some(holon_api::Value::String(s)) => s.clone(),
            other => panic!("unexpected id value: {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn recursive_cte_stops_at_non_root_pages() {
    let handle = setup().await;

    // Minimal tree: root -> child-page -> grandchild
    insert_block(&handle, "root", "sentinel:no_parent", "Root", "A0").await;
    insert_block(&handle, "child", "root", "Child Page", "A1").await;
    insert_block(&handle, "grandchild", "child", "Grandchild", "A2").await;
    tag_page(&handle, "child").await;

    set_focus(&handle, "main", "root").await;
    reconcile_named_view(&handle, "focus_descendants_view", MAIN_PANEL_CTE)
        .await
        .expect("create matview");

    let ids = read_descendants(&handle).await;
    // root + child (page boundary: grandchild excluded)
    assert_eq!(ids, vec!["root".to_string(), "child".to_string()],
        "expected root + child only (grandchild blocked by page boundary)");
}

#[tokio::test]
async fn recursive_cte_includes_children_of_non_page_intermediaries() {
    let handle = setup().await;

    // root -> folder (non-page) -> nested-page -> deep-child
    insert_block(&handle, "root", "sentinel:no_parent", "Root", "A0").await;
    insert_block(&handle, "folder", "root", "Folder", "A1").await;
    insert_block(&handle, "nested", "folder", "Nested Page", "A2").await;
    insert_block(&handle, "deep", "nested", "Deep Child", "A3").await;
    tag_page(&handle, "nested").await;

    set_focus(&handle, "main", "root").await;
    reconcile_named_view(&handle, "focus_descendants_folder", MAIN_PANEL_CTE)
        .await
        .expect("create matview");

    let ids = read_descendants_alias(&handle, "focus_descendants_folder").await;
    // root + folder + nested (boundary at nested; deep excluded)
    assert_eq!(ids, vec!["root".to_string(), "folder".to_string(), "nested".to_string()],
        "expected root + folder + nested only (deep blocked by page boundary at nested)");
}

async fn read_descendants_alias(handle: &DbHandle, view_name: &str) -> Vec<String> {
    let sql = format!("SELECT id FROM {view_name} ORDER BY _depth, sort_key");
    let rows = handle.query(&sql, HashMap::new()).await.expect("query view");
    rows.iter()
        .map(|r| match r.get("id") {
            Some(holon_api::Value::String(s)) => s.clone(),
            other => panic!("unexpected id value: {other:?}"),
        })
        .collect()
}

#[tokio::test]
async fn recursive_cte_ivm_updates_on_tag_insert() {
    let handle = setup().await;

    // Deep tree where a middle node gets tagged as Page mid-test
    insert_block(&handle, "root", "sentinel:no_parent", "Root", "A0").await;
    insert_block(&handle, "mid", "root", "Mid", "A1").await;
    insert_block(&handle, "leaf", "mid", "Leaf", "A2").await;
    // NOT tagged yet — mid is not a page initially

    set_focus(&handle, "main", "root").await;
    reconcile_named_view(&handle, "focus_descendants_ivm", MAIN_PANEL_CTE)
        .await
        .expect("create matview");

    // Before tagging: all three visible
    let ids = read_descendants_alias(&handle, "focus_descendants_ivm").await;
    assert_eq!(ids, vec!["root", "mid", "leaf"],
        "before tagging: all blocks visible");

    // Tag mid as Page — IVM must remove leaf from the result
    tag_page(&handle, "mid").await;

    // Give IVM a tick to propagate
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let ids = read_descendants_alias(&handle, "focus_descendants_ivm").await;
    assert_eq!(ids, vec!["root", "mid"],
        "after tagging mid as Page: leaf must be excluded (page boundary at mid)");
}

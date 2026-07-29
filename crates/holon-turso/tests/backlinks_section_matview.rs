//! IVM verification for the shipped "Linked references" accordion: the
//! `backlinks` matview and the watch matview the seeded `live_query` builds on
//! top of it (`assets/default/index.org`).
//!
//! Two things are load-bearing and neither is covered by the headless keystone
//! (which never renders the accordion):
//!   - the widened `backlinks` projection (the whole source-block row, so the
//!     `block` entity profile's computed fields bind) is IVM-creatable;
//!   - `SELECT bl.*` over that matview, joined to two further relations, still
//!     delivers the block columns the profile declares.

use std::collections::HashMap;

use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::schema_modules::backlinks_view_select;
use holon_turso::schema_modules::block_raw_schema_sql;
use holon_turso::sql_utils::sql_statements;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// The seeded accordion query with its `ORDER BY` stripped — matview DDL
/// rejects a column-reference ORDER BY, and the production watch path strips it
/// the same way.
const SECTION_SQL: &str = "SELECT bl.* FROM backlinks bl JOIN focus_roots fr ON bl.target_id = \
                           fr.root_id JOIN navigation_cursor nc ON nc.region = fr.region AND \
                           nc.history_id = fr.history_id WHERE fr.region = 'main'";

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);

    for stmt in sql_statements(block_raw_schema_sql()) {
        handle.execute_ddl(stmt).await.expect("block_raw schema");
    }
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id) VALUES ('sentinel:no_parent', \
             'sentinel:no_parent')",
            vec![],
        )
        .await
        .expect("seed FK sentinel");
    handle
        .execute_ddl(
            "CREATE TABLE block_links (source_block_id TEXT NOT NULL, target_text TEXT NOT NULL, \
             resolved_id TEXT, PRIMARY KEY (source_block_id, target_text))",
        )
        .await
        .expect("create block_links");
    handle
        .execute_ddl(
            "CREATE TABLE focus_roots (region TEXT NOT NULL, root_id TEXT NOT NULL, added_ts TEXT, \
             history_id INTEGER)",
        )
        .await
        .expect("create focus_roots");
    handle
        .execute_ddl("CREATE TABLE navigation_cursor (region TEXT PRIMARY KEY, history_id INTEGER)")
        .await
        .expect("create navigation_cursor");
    handle
}

async fn insert_source_block(handle: &DbHandle, id: &str, language: &str, collapsed: i64) {
    handle
        .execute(
            "INSERT INTO block_raw (id, parent_id, content, content_type, source_language, \
             collapsed) VALUES (?, 'sentinel:no_parent', 'ref', 'source', ?, ?)",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Text(language.into()),
                turso::Value::Integer(collapsed),
            ],
        )
        .await
        .expect("insert block_raw");
}

/// The accordion's rows must carry `collapsed` and `source_language` — without
/// them `bullet_shape`, `is_rule_head`, `is_holon_source` and `is_legacy_rule`
/// are structurally unbound and every backlink renders as an unclassified
/// plain-dot row (dogfood 2026-07-28).
#[tokio::test]
async fn section_matview_delivers_the_columns_the_block_profile_declares() {
    let handle = setup().await;

    reconcile_named_view(&handle, "backlinks", &backlinks_view_select())
        .await
        .expect("backlinks matview must be IVM-creatable with the full block row");
    reconcile_named_view(&handle, "section_view", SECTION_SQL)
        .await
        .expect("accordion watch view must be IVM-creatable over backlinks");

    insert_source_block(&handle, "block:src", "holon_rule", 1).await;
    insert_source_block(&handle, "block:target", "holon_sql", 0).await;
    handle
        .execute(
            "INSERT INTO block_links (source_block_id, target_text, resolved_id) VALUES \
             ('block:src', 'target', 'block:target')",
            vec![],
        )
        .await
        .expect("insert link");
    handle
        .execute(
            "INSERT INTO focus_roots (region, root_id, added_ts, history_id) VALUES ('main', \
             'block:target', '0', 1)",
            vec![],
        )
        .await
        .expect("insert focus root");
    handle
        .execute(
            "INSERT INTO navigation_cursor (region, history_id) VALUES ('main', 1)",
            vec![],
        )
        .await
        .expect("insert cursor");

    let rows = handle
        .query("SELECT * FROM section_view", HashMap::new())
        .await
        .expect("read section view");
    assert_eq!(rows.len(), 1, "one incoming link to the focused page");
    let row = &rows[0];
    let id = row.get("id").and_then(|v| v.as_string());
    assert_eq!(
        id,
        Some("block:src"),
        "row identity is the LINKING block: {row:?}"
    );
    for column in ["collapsed", "source_language", "content_type", "parent_id"] {
        assert!(
            row.contains_key(column),
            "accordion row must carry declared block column '{column}': {row:?}"
        );
    }
    let language = row.get("source_language").and_then(|v| v.as_string());
    assert_eq!(
        language,
        Some("holon_rule"),
        "source_language must survive the two joins: {row:?}"
    );
}

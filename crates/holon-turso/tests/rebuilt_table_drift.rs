//! Contract: a stored block-tree table whose shape no column addition fixes
//! keeps every row at boot: its rows are copied into the declared shape,
//! because without the Loro store it holds the only durable copy of the block
//! tree. A row the declaration cannot hold fails the schema module and leaves
//! every stored table as it was.

use std::collections::HashMap;
use std::path::Path;

use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::block_raw_schema_sql;
use holon_turso::sql_utils::sql_statements;
use holon_turso::table_shape::TableChange;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

async fn open(path: &Path) -> (TursoBackend, DbHandle) {
    let db = TursoBackend::open_database(path).expect("open");
    TursoBackend::new(db, broadcast::channel(64).0).expect("backend")
}

async fn ids(handle: &DbHandle, sql: &str) -> Vec<String> {
    handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .into_iter()
        .map(|r| format!("{:?}", r.values().next().expect("one column")))
        .collect()
}

async fn columns(handle: &DbHandle, table: &str) -> Vec<String> {
    handle
        .query(&format!("PRAGMA table_info({table})"), HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("PRAGMA table_info({table}): {e}"))
        .into_iter()
        .map(|r| format!("{:?}", r.get("name").expect("a column name")))
        .collect()
}

/// A database whose `block_raw` is declared by `drifted` and holds the
/// `rows`, plus a tag and a dismissal of `b1`, an ingested file, and a matview
/// over `block_raw`.
async fn drifted_database(path: &Path, drifted: &str, rows: &[&str]) {
    let (_b, handle) = open(path).await;
    handle
        .execute_ddl("PRAGMA foreign_keys = ON")
        .await
        .expect("fk on");
    for sql in [
        drifted,
        include_str!("../sql/schema/files.sql"),
        include_str!("../sql/schema/block_tags.sql"),
        include_str!("../sql/schema/advice_suppressed.sql"),
    ] {
        for stmt in sql_statements(sql) {
            handle.execute_ddl(stmt).await.expect(stmt);
        }
    }
    for stmt in rows.iter().copied().chain([
        "INSERT INTO block_tags (block_id, tag) VALUES ('b1', 't')",
        "INSERT INTO advice_suppressed (anchor_id, lesson_id) VALUES ('b1', 'l1')",
        "INSERT INTO file (id, name, parent_id, content_hash) VALUES ('f1', 'a.org', 'root', 'h')",
    ]) {
        handle.execute(stmt, vec![]).await.expect(stmt);
    }
    handle
        .execute_ddl("CREATE MATERIALIZED VIEW block AS SELECT id, content FROM block_raw")
        .await
        .expect("matview over block_raw");
    handle.shutdown().await.expect("shutdown");
}

/// The rows of every block-tree table the drifted databases seed.
async fn block_tree_rows(handle: &DbHandle) -> Vec<(&'static str, usize)> {
    let mut counts = Vec::new();
    for (table, sql) in [
        ("block_raw", "SELECT id FROM block_raw"),
        ("block_tags", "SELECT tag FROM block_tags"),
        ("file", "SELECT id FROM file"),
        (
            "advice_suppressed",
            "SELECT lesson_id FROM advice_suppressed",
        ),
    ] {
        counts.push((table, ids(handle, sql).await.len()));
    }
    counts
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drifted_block_raw_keeps_every_row_in_its_declared_shape() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("old.db");
    let drifted = block_raw_schema_sql().replacen(
        "CREATE TABLE IF NOT EXISTS block_raw (",
        "CREATE TABLE IF NOT EXISTS block_raw (legacy TEXT NOT NULL,",
        1,
    );
    drifted_database(
        &path,
        &drifted,
        &[
            "INSERT INTO block_raw (id, parent_id, legacy) VALUES ('sentinel:no_parent', \
             'sentinel:no_parent', 'x')",
            "INSERT INTO block_raw (id, parent_id, content, block_type, legacy) VALUES ('b1', \
             'sentinel:no_parent', 'kept', 'page', 'x')",
        ],
    )
    .await;

    let (_b, handle) = open(&path).await;
    let changes = CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("a drifted block_raw does not fail the core tables");
    let reshaped: Vec<_> = changes
        .iter()
        .filter_map(|c| match c {
            TableChange::Reshaped(r) => Some((r.diff.table.as_str(), r.rows)),
            TableChange::Rebuilt(_) | TableChange::ColumnsAdded(_) => None,
        })
        .collect();
    assert_eq!(reshaped, vec![("block_raw", 2)], "changes: {changes:?}");
    assert!(
        !changes.iter().any(|c| matches!(c, TableChange::Rebuilt(_))),
        "no block-tree table is dropped: {changes:?}"
    );
    assert!(
        !columns(&handle, "block_raw")
            .await
            .iter()
            .any(|c| c.contains("legacy")),
        "block_raw has its declared shape"
    );
    assert_eq!(
        ids(
            &handle,
            "SELECT content || '/' || block_type FROM block_raw WHERE id = 'b1'"
        )
        .await,
        vec![format!(
            "{:?}",
            holon_api::Value::String("kept/page".into())
        )],
        "b1 keeps its values"
    );
    assert_eq!(
        block_tree_rows(&handle).await,
        vec![
            ("block_raw", 2),
            ("block_tags", 1),
            ("file", 1),
            ("advice_suppressed", 1)
        ],
        "reshaping block_raw keeps every row of the block tree"
    );

    let again = CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("the reshaped block_raw boots again");
    assert!(again.is_empty(), "the reshape is stable: {again:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_row_the_declaration_cannot_hold_keeps_the_stored_block_raw() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("old.db");
    let declared = "sort_key TEXT NOT NULL DEFAULT 'A0',";
    assert!(block_raw_schema_sql().contains(declared));
    let drifted = block_raw_schema_sql().replacen(declared, "sort_key TEXT,", 1);
    drifted_database(
        &path,
        &drifted,
        &[
            "INSERT INTO block_raw (id, parent_id, sort_key) VALUES ('sentinel:no_parent', \
             'sentinel:no_parent', 'A0')",
            "INSERT INTO block_raw (id, parent_id, sort_key) VALUES ('b1', 'sentinel:no_parent', \
             NULL)",
        ],
    )
    .await;

    let (_b, handle) = open(&path).await;
    let error = CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect_err("a row block_raw's declaration cannot hold must fail the core tables")
        .to_string();
    assert!(
        error.contains("block_raw") && error.contains("sort_key"),
        "the error must name the table and the difference: {error}"
    );
    assert_eq!(
        block_tree_rows(&handle).await,
        vec![
            ("block_raw", 2),
            ("block_tags", 1),
            ("file", 1),
            ("advice_suppressed", 1)
        ],
        "a refused reshape leaves every row where it was"
    );
    assert!(
        columns(&handle, "block_raw")
            .await
            .iter()
            .any(|c| c.contains("sort_key")),
        "the stored block_raw is still there"
    );
    assert!(
        ids(
            &handle,
            "SELECT name FROM sqlite_master WHERE name LIKE 'block_raw%' AND type = 'table'"
        )
        .await
            == vec![format!(
                "{:?}",
                holon_api::Value::String("block_raw".into())
            )],
        "no scratch table is left behind"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_reshape_keeps_the_views_over_the_stored_table() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("old.db");
    let declared = "sort_key TEXT NOT NULL DEFAULT 'A0',";
    let drifted = block_raw_schema_sql().replacen(declared, "sort_key TEXT,", 1);
    drifted_database(
        &path,
        &drifted,
        &[
            "INSERT INTO block_raw (id, parent_id, sort_key) VALUES ('sentinel:no_parent', \
             'sentinel:no_parent', 'A0')",
            "INSERT INTO block_raw (id, parent_id, sort_key) VALUES ('b1', 'sentinel:no_parent', \
             NULL)",
        ],
    )
    .await;

    let (_b, handle) = open(&path).await;
    for boot in 1..=2 {
        CoreSchemaModule
            .ensure_schema(&handle)
            .await
            .expect_err("a row block_raw's declaration cannot hold must fail the core tables");
        assert_eq!(
            ids(&handle, "SELECT id FROM block ORDER BY id").await.len(),
            2,
            "boot {boot}: the block matview still serves the stored rows"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_rebuild_whose_declaration_cannot_be_created_keeps_the_stored_table() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_b, handle) = open(&dir.path().join("old.db")).await;
    let drifted = block_raw_schema_sql().replacen(
        "CREATE TABLE IF NOT EXISTS block_raw (",
        "CREATE TABLE IF NOT EXISTS block_raw (legacy TEXT NOT NULL,",
        1,
    );
    for sql in [
        drifted.as_str(),
        include_str!("../sql/schema/files.sql"),
        include_str!("../sql/schema/block_tags.sql"),
    ] {
        for stmt in sql_statements(sql) {
            handle.execute_ddl(stmt).await.expect(stmt);
        }
    }
    for stmt in [
        "INSERT INTO block_raw (id, parent_id, legacy) VALUES ('sentinel:no_parent', \
         'sentinel:no_parent', 'x')",
        "INSERT INTO block_raw (id, parent_id, legacy) VALUES ('b1', 'sentinel:no_parent', 'x')",
        "INSERT INTO block_tags (block_id, tag) VALUES ('b1', 't')",
        "INSERT INTO file (id, name, parent_id, content_hash) VALUES ('f1', 'a.org', 'root', 'h')",
    ] {
        handle.execute(stmt, vec![]).await.expect(stmt);
    }

    // Passes `IF NOT EXISTS` against the stored table; only a fresh CREATE
    // finds the unknown key column.
    let uncreatable =
        "CREATE TABLE IF NOT EXISTS block_raw (id TEXT, parent_id TEXT, PRIMARY KEY (nope))";
    let error = holon_turso::table_shape::ensure_statement(&handle, uncreatable)
        .await
        .expect_err("a declaration the engine cannot create must fail the rebuild")
        .to_string();
    assert!(
        error.contains("block_raw") && error.contains("nope"),
        "the error must name the table and why its declaration fails: {error}"
    );
    for (sql, rows) in [
        ("SELECT id FROM block_raw", 2),
        ("SELECT tag FROM block_tags", 1),
        ("SELECT id FROM file", 1),
    ] {
        assert_eq!(
            ids(&handle, sql).await.len(),
            rows,
            "{sql}: a failed rebuild must leave every row where it was"
        );
    }
}

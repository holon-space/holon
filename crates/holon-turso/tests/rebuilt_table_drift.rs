//! Contract: a stored table the org files and the Loro store refill, whose
//! shape no column addition fixes, is dropped and recreated empty at boot
//! instead of failing its schema module, and the record of what was already
//! ingested into it is emptied so the next ingest refills it. Rows of tables
//! nothing refills stay where they are.

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

#[tokio::test(flavor = "multi_thread")]
async fn a_drifted_block_raw_is_rebuilt_and_its_ingest_record_emptied() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("old.db");
    {
        let (_b, handle) = open(&path).await;
        handle
            .execute_ddl("PRAGMA foreign_keys = ON")
            .await
            .expect("fk on");
        let drifted = block_raw_schema_sql().replacen(
            "CREATE TABLE IF NOT EXISTS block_raw (",
            "CREATE TABLE IF NOT EXISTS block_raw (legacy TEXT NOT NULL,",
            1,
        );
        for sql in [
            drifted.as_str(),
            include_str!("../sql/schema/files.sql"),
            include_str!("../sql/schema/block_tags.sql"),
            include_str!("../sql/schema/advice_suppressed.sql"),
        ] {
            for stmt in sql_statements(sql) {
                handle.execute_ddl(stmt).await.expect(stmt);
            }
        }
        for stmt in [
            "INSERT INTO block_raw (id, parent_id, legacy) VALUES ('sentinel:no_parent', \
             'sentinel:no_parent', 'x')",
            "INSERT INTO block_raw (id, parent_id, legacy) VALUES ('b1', 'sentinel:no_parent', \
             'x')",
            "INSERT INTO block_tags (block_id, tag) VALUES ('b1', 't')",
            "INSERT INTO advice_suppressed (anchor_id, lesson_id) VALUES ('b1', 'l1')",
            "INSERT INTO file (id, name, parent_id, content_hash) VALUES ('f1', 'a.org', 'root', \
             'h')",
        ] {
            handle.execute(stmt, vec![]).await.expect(stmt);
        }
        handle
            .execute_ddl("CREATE MATERIALIZED VIEW block AS SELECT id, content FROM block_raw")
            .await
            .expect("matview over block_raw");
        handle.shutdown().await.expect("shutdown");
    }

    let (_b, handle) = open(&path).await;
    let changes = CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("a drifted block_raw does not fail the core tables");
    let rebuilt: Vec<_> = changes
        .iter()
        .filter_map(|c| match c {
            TableChange::Rebuilt(r) => Some((r.diff.table.as_str(), r.rows)),
            TableChange::ColumnsAdded(_) => None,
        })
        .collect();
    assert_eq!(rebuilt, vec![("block_raw", 2)], "changes: {changes:?}");
    assert_eq!(
        ids(&handle, "SELECT id FROM block_raw").await,
        vec![format!(
            "{:?}",
            holon_api::Value::String("sentinel:no_parent".into())
        )],
        "only the re-seeded sentinel remains"
    );
    assert!(
        ids(&handle, "SELECT id FROM file").await.is_empty(),
        "the ingest record is emptied so every org file is ingested again"
    );
    assert!(
        ids(&handle, "SELECT tag FROM block_tags").await.is_empty(),
        "the junctions the same ingest refills are emptied with block_raw"
    );
    assert_eq!(
        ids(&handle, "SELECT lesson_id FROM advice_suppressed")
            .await
            .len(),
        1,
        "dismissed advice is nothing the org files restore, so it is kept"
    );
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

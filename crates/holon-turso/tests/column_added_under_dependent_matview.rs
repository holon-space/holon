//! Contract: a hand-written table that predates a declared column gains it at
//! boot even when a materialized view over the table is stored on disk.
//!
//! On a real database the `block` matview reads `block_raw` and the sidebar's
//! watch view reads `integration_state`, and the engine refuses every
//! `ALTER TABLE` on a table with a dependent matview.

use std::collections::HashMap;
use std::path::Path;

use holon_api::Value;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::schema_modules::IntegrationStateSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

async fn open(path: &Path) -> (TursoBackend, DbHandle) {
    let db = TursoBackend::open_database(path).expect("open");
    TursoBackend::new(db, broadcast::channel(64).0).expect("backend")
}

async fn columns(handle: &DbHandle, table: &str) -> Vec<String> {
    handle
        .query(&format!("PRAGMA table_info({table})"), HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("PRAGMA table_info({table}): {e}"))
        .into_iter()
        .map(|r| match r.get("name") {
            Some(Value::String(name)) => name.clone(),
            other => panic!("column name {other:?}"),
        })
        .collect()
}

async fn count(handle: &DbHandle, table: &str) -> usize {
    handle
        .query(&format!("SELECT id FROM {table}"), HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("count {table}: {e}"))
        .len()
}

/// Seed `ddl` (old-shape table, its rows, and a matview over it), reopen, and
/// run `module` the way boot does.
async fn boot_over(ddl: &[&str], module: &dyn SchemaModule) -> (TursoBackend, DbHandle) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.keep().join("old.db");
    {
        let (_b, handle) = open(&path).await;
        for stmt in ddl {
            handle
                .execute_ddl(stmt)
                .await
                .unwrap_or_else(|e| panic!("seed {stmt}: {e}"));
        }
        handle.shutdown().await.expect("shutdown");
    }
    let (backend, handle) = open(&path).await;
    module
        .ensure_schema(&handle)
        .await
        .unwrap_or_else(|e| panic!("booting {} over a stored matview: {e}", module.name()));
    (backend, handle)
}

#[tokio::test(flavor = "multi_thread")]
async fn block_raw_gains_property_kinds_under_the_block_matview() {
    let (_b, handle) = boot_over(
        &[
            "CREATE TABLE block_raw (id TEXT PRIMARY KEY, parent_id TEXT, sort_key TEXT NOT NULL \
             DEFAULT 'A0', content TEXT NOT NULL DEFAULT '', content_type TEXT NOT NULL DEFAULT \
             'text', source_language TEXT, source_name TEXT, properties TEXT, marks TEXT, \
             collapsed INTEGER NOT NULL DEFAULT 0, widget_only INTEGER NOT NULL DEFAULT 0, \
             block_type TEXT, created_at INTEGER NOT NULL DEFAULT 0, updated_at INTEGER NOT \
             NULL DEFAULT 0, _change_origin TEXT, write_seq INTEGER NOT NULL DEFAULT 0)",
            "INSERT INTO block_raw (id, parent_id) VALUES ('b1', 'b1')",
            "CREATE MATERIALIZED VIEW block AS SELECT id, parent_id, content FROM block_raw",
        ],
        &CoreSchemaModule,
    )
    .await;
    assert!(
        columns(&handle, "block_raw")
            .await
            .contains(&"property_kinds".to_string())
    );
    assert_eq!(
        count(&handle, "block_raw").await,
        2,
        "b1 plus the sentinel row"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn file_gains_read_only_blocks_under_a_stored_matview() {
    let (_b, handle) = boot_over(
        &[
            "CREATE TABLE file (id TEXT PRIMARY KEY NOT NULL, name TEXT NOT NULL, parent_id TEXT \
             NOT NULL, content_hash TEXT NOT NULL DEFAULT '', document_id TEXT, properties TEXT, \
             property_kinds TEXT, _change_origin TEXT)",
            "INSERT INTO file (id, name, parent_id) VALUES ('f1', 'a.org', 'root')",
            "CREATE MATERIALIZED VIEW watch_view_files AS SELECT id, name FROM file",
        ],
        &CoreSchemaModule,
    )
    .await;
    assert!(
        columns(&handle, "file")
            .await
            .contains(&"read_only_blocks".to_string())
    );
    assert_eq!(count(&handle, "file").await, 1);
}

#[tokio::test(flavor = "multi_thread")]
async fn integration_state_gains_its_presentation_columns_under_the_sidebar_matview() {
    let (_b, handle) = boot_over(
        &[
            "CREATE TABLE integration_state (id TEXT PRIMARY KEY NOT NULL, provider_name TEXT NOT \
             NULL, enabled INTEGER NOT NULL, status TEXT NOT NULL, config_status TEXT NOT NULL, \
             configurable INTEGER NOT NULL, configure_progress TEXT NOT NULL, updated_at TEXT NOT \
             NULL, _change_origin TEXT)",
            "INSERT INTO integration_state (id, provider_name, enabled, status, config_status, \
             configurable, configure_progress, updated_at) VALUES ('todoist', 'todoist', 1, \
             'ok', 'ok', 1, '', '')",
            "CREATE MATERIALIZED VIEW watch_view_integrations AS SELECT id, provider_name, status \
             FROM integration_state WHERE enabled = 1",
        ],
        &IntegrationStateSchemaModule,
    )
    .await;
    let present = columns(&handle, "integration_state").await;
    for column in ["display_name", "icon", "default_view", "origin", "hosts"] {
        assert!(
            present.contains(&column.to_string()),
            "{column} missing: {present:?}"
        );
    }
    assert_eq!(count(&handle, "integration_state").await, 1);
}

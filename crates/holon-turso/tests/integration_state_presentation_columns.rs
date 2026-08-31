//! Contract: a database written before `integration_state` grew its
//! presentation axis boots into the current shape, keeping its rows.
//!
//! `CREATE TABLE IF NOT EXISTS` leaves an existing table alone, so without a
//! migration every vault that ever booted an earlier build would keep a
//! six-column mirror and every projector write would die on the unknown column
//! — a working app that stops working after an update, which is the failure
//! this rung exists to prevent.
//!
//! @pbt kind harness
//! @pbt covers integration-state-presentation-columns — `display_name`, `icon`
//! and `default_view` are present after boot on a fresh AND on a pre-existing
//! database, and the migration is idempotent across reboots

use std::collections::HashMap;

use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::IntegrationStateSchemaModule;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

const PRESENTATION: &[&str] = &["display_name", "icon", "default_view"];

/// The shape the table had before the presentation axis: the CREATE that
/// shipped, restated so this rung keeps describing the OLD database after the
/// current DDL moves on.
const OLD_SHAPE: &str = "CREATE TABLE integration_state (\
     id TEXT PRIMARY KEY NOT NULL, \
     provider_name TEXT NOT NULL, \
     enabled INTEGER NOT NULL, \
     status TEXT NOT NULL, \
     config_status TEXT NOT NULL, \
     configurable INTEGER NOT NULL, \
     configure_progress TEXT NOT NULL, \
     updated_at TEXT NOT NULL, \
     _change_origin TEXT)";

async fn open(path: &std::path::Path) -> (TursoBackend, holon_turso::turso::DbHandle) {
    let db = TursoBackend::open_database(path).expect("open database");
    TursoBackend::new(db, broadcast::channel(64).0).expect("backend")
}

async fn columns(handle: &holon_turso::turso::DbHandle) -> Vec<String> {
    handle
        .query("PRAGMA table_info(integration_state)", HashMap::new())
        .await
        .expect("read the column set")
        .iter()
        .map(|r| {
            r.get("name")
                .and_then(|v| v.as_string())
                .expect("table_info projects name")
                .to_string()
        })
        .collect()
}

fn assert_carries_the_presentation_axis(columns: &[String], context: &str) {
    for wanted in PRESENTATION {
        assert!(
            columns.iter().any(|c| c == wanted),
            "{context}: integration_state must carry `{wanted}`; it has {columns:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_database_is_created_with_the_presentation_axis() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_backend, handle) = open(&dir.path().join("fresh.db")).await;

    IntegrationStateSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("fresh schema");

    assert_carries_the_presentation_axis(&columns(&handle).await, "fresh database");
}

/// The migration leg, and the reason it exists: the row already in the table
/// must still be there afterwards. Dropping and recreating would lose the
/// enablement mirror until the next projection, so the columns are appended.
#[tokio::test(flavor = "multi_thread")]
async fn a_pre_existing_database_gains_the_columns_and_keeps_its_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_backend, handle) = open(&dir.path().join("old.db")).await;

    handle.execute_ddl(OLD_SHAPE).await.expect("old shape");
    handle
        .execute_values(
            "INSERT INTO integration_state \
             (id, provider_name, enabled, status, config_status, configurable, \
             configure_progress, updated_at) \
             VALUES ('integration:todoist', 'todoist', 1, 'Connected', 'configured', 0, '', \
             '2026-01-01 00:00:00')",
            vec![],
        )
        .await
        .expect("seed a row in the old shape");

    IntegrationStateSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("the migration must run against the old shape");

    assert_carries_the_presentation_axis(&columns(&handle).await, "migrated database");

    let rows = handle
        .query(
            "SELECT provider_name FROM integration_state",
            HashMap::new(),
        )
        .await
        .expect("read the surviving rows");
    assert_eq!(
        rows.len(),
        1,
        "the migration must keep the row it found, not rebuild the table"
    );

    // Twice, because `ensure_schema` runs on every boot and an ALTER that
    // re-fires dies with `duplicate column name`.
    IntegrationStateSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("the migration must be a no-op the second time");
    assert_carries_the_presentation_axis(&columns(&handle).await, "second boot");
}

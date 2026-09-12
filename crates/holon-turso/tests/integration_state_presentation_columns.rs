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
//! @pbt covers integration-state-presentation-columns — `display_name`, `icon`,
//! `default_view`, `origin` and `hosts` are present after boot on a fresh AND
//! on a pre-existing database, and the migration is idempotent across reboots

use std::collections::HashMap;

use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::IntegrationStateSchemaModule;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast;

const PRESENTATION: &[&str] = &["display_name", "icon", "default_view"];

/// The DISCLOSURE axis, added after the presentation one and migrated by the
/// same additive step. Checked here rather than in a rung of its own because
/// the failure mode is identical — an un-migrated vault whose projector writes
/// die on an unknown column — and one rung that names every appended column
/// cannot fall behind the next addition the way a per-axis copy would.
const DISCLOSURE: &[&str] = &["origin", "hosts"];

/// Every column the current DDL appends to a database that predates it.
fn migrated_columns() -> impl Iterator<Item = &'static &'static str> {
    PRESENTATION.iter().chain(DISCLOSURE.iter())
}

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
    for wanted in migrated_columns() {
        assert!(
            columns.iter().any(|c| c == wanted),
            "{context}: integration_state must carry `{wanted}`; it has {columns:?}"
        );
    }
}

/// A fresh database and a migrated one must accept the SAME inserts.
///
/// They did not. Every appended column is declared `NOT NULL DEFAULT ''` by the
/// migration, while the CREATE declared `display_name` and `icon` `NOT NULL`
/// with no default — so an insert that omitted them succeeded on a vault that
/// had been upgraded and failed on a vault created today. Two fixtures in
/// `holon-integration-tests` hit exactly that
/// (`NOT NULL constraint failed: integration_state.display_name`), and the
/// defect is worse than the fixtures: it makes a bug reproducible only on
/// machines whose database has the right history.
///
/// Asserted as AGREEMENT rather than by restating either DDL — the property is
/// that the two paths converge, not what they converge on.
#[tokio::test(flavor = "multi_thread")]
async fn a_fresh_and_a_migrated_database_accept_the_same_insert() {
    let dir = tempfile::tempdir().expect("tempdir");

    let (_fresh_backend, fresh) = open(&dir.path().join("fresh.db")).await;
    IntegrationStateSchemaModule
        .ensure_schema(&fresh)
        .await
        .expect("fresh schema");

    let (_old_backend, migrated) = open(&dir.path().join("old.db")).await;
    migrated.execute_ddl(OLD_SHAPE).await.expect("old shape");
    IntegrationStateSchemaModule
        .ensure_schema(&migrated)
        .await
        .expect("migration");

    // The minimum a caller can name: the columns the ORIGINAL table required.
    // Everything appended since must carry a default, or this insert is a
    // coin-flip on the database's history.
    const MINIMAL: &str = "INSERT INTO integration_state          (id, provider_name, enabled, status, config_status, configurable,          configure_progress, updated_at)          VALUES ('integration:x', 'x', 1, 'Connected', 'configured', 0, '', '2026-01-01')";

    let on_migrated = migrated.execute_values(MINIMAL, vec![]).await;
    let on_fresh = fresh.execute_values(MINIMAL, vec![]).await;

    assert_eq!(
        on_fresh.is_ok(),
        on_migrated.is_ok(),
        "a fresh database and a migrated one must accept the same insert, or a defect reproduces          only on machines whose vault has the right history. fresh: {:?} / migrated: {:?}",
        on_fresh.as_ref().err().map(|e| e.to_string()),
        on_migrated.as_ref().err().map(|e| e.to_string()),
    );
    on_fresh
        .expect("and the shared answer must be `accepted` — every appended column has a default");
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

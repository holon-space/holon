//! Generic conformance suite for EVERY shipped integration sidecar.
//!
//! These tests are provider-agnostic: they enumerate
//! `assets/integrations/*.yaml` and assert the invariants that must hold for
//! ANY sidecar, so a NEW sidecar (drop a `*.yaml` in that directory) is
//! automatically validated with no new test code. Each assertion names the
//! offending file + entity/view, so a failure points straight at the culprit.
//!
//! Invariants checked:
//!   1. parses strictly as `IntegrationFileConfig` (`deny_unknown_fields`
//!      catches typo'd keys);
//!   2. every entity's `schema` produces a CREATE TABLE that a real Turso
//!      accepts (guards, e.g., unquoted SQL-keyword columns);
//!   3. every `views:` entry is IVM-valid — it CREATEs as a materialized view
//!      (in declared order, so chained views resolve), exactly as
//!      `finish_integration` does at connect;
//!   4. for `rest` sidecars, every entity's `sync.list_tool` names a declared
//!      `transport.rest.calls` entry, every `project` target is a declared
//!      schema column, and `pagination.max_pages >= 1`.
//!
//! Provider-SPECIFIC behavior (e.g. gcal's exact +7d upcoming window, or its
//! oauth2 contract) stays in focused per-provider tests; this suite is only the
//! structural floor every sidecar must clear.

use std::path::Path;
use std::path::PathBuf;

use holon_mcp_client::IntegrationFileConfig;
use holon_mcp_client::McpSidecar;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

/// Absolute paths of every shipped sidecar YAML.
fn sidecar_paths() -> Vec<PathBuf> {
    let dir = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/integrations"
    ));
    let mut out: Vec<PathBuf> = std::fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("read {}: {e}", dir.display()))
        .map(|e| e.expect("dir entry").path())
        .filter(|p| {
            matches!(
                p.extension().and_then(|e| e.to_str()),
                Some("yaml") | Some("yml")
            )
        })
        .collect();
    out.sort();
    assert!(
        !out.is_empty(),
        "no sidecar YAMLs found in {}",
        dir.display()
    );
    out
}

fn read(path: &PathBuf) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn name_of(path: &Path) -> String {
    path.file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("?")
        .to_string()
}

async fn fresh_db() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    // Leak the backend so its actor outlives the handle for the test.
    std::mem::forget(backend);
    handle
}

/// Create every entity's cache table (+ indexes) from its schema. Returns the
/// sidecar for follow-on view reconciliation. Panics (naming the file/entity)
/// on invalid DDL.
async fn create_all_cache_tables(file: &str, sidecar: &McpSidecar, db: &DbHandle) {
    for (entity_name, entity) in &sidecar.entities {
        let table = sidecar.prefixed_name(entity_name).table_name();
        let Some(td) = entity.to_type_definition(&table) else {
            continue; // schema-less entity: nothing to create
        };
        let ddl = td.to_create_table_sql();
        db.execute_ddl(&ddl).await.unwrap_or_else(|e| {
            panic!("[{file}] entity '{entity_name}' invalid CREATE TABLE: {e}\nDDL: {ddl}")
        });
        for index_sql in td.to_index_sql() {
            db.execute_ddl(&index_sql).await.unwrap_or_else(|e| {
                panic!("[{file}] entity '{entity_name}' invalid index DDL: {e}\n{index_sql}")
            });
        }
    }
}

#[test]
fn every_sidecar_parses_strictly() {
    for path in sidecar_paths() {
        let file = name_of(&path);
        let yaml = read(&path);
        serde_yaml::from_str::<IntegrationFileConfig>(&yaml)
            .unwrap_or_else(|e| panic!("[{file}] does not parse as IntegrationFileConfig: {e}"));
    }
}

#[tokio::test]
async fn every_sidecar_schema_creates_valid_cache_tables() {
    for path in sidecar_paths() {
        let file = name_of(&path);
        let sidecar = McpSidecar::from_yaml(&read(&path))
            .unwrap_or_else(|e| panic!("[{file}] McpSidecar parse: {e}"));
        let db = fresh_db().await;
        create_all_cache_tables(&file, &sidecar, &db).await;
    }
}

#[tokio::test]
async fn every_sidecar_view_is_ivm_valid() {
    for path in sidecar_paths() {
        let file = name_of(&path);
        let sidecar = McpSidecar::from_yaml(&read(&path))
            .unwrap_or_else(|e| panic!("[{file}] McpSidecar parse: {e}"));
        if sidecar.views.is_empty() {
            continue;
        }
        let db = fresh_db().await;
        create_all_cache_tables(&file, &sidecar, &db).await;
        // Reconcile in declared order so chained views resolve — mirrors
        // finish_integration::reconcile_sidecar_views.
        for view in &sidecar.views {
            let view_name = sidecar.prefixed_name(&view.name).table_name();
            let created = reconcile_named_view(&db, &view_name, &view.sql)
                .await
                .unwrap_or_else(|e| {
                    panic!(
                        "[{file}] view '{}' (matview '{view_name}') is not IVM-valid: {e}\nSQL: {}",
                        view.name, view.sql
                    )
                });
            assert!(
                created,
                "[{file}] first reconcile of '{view_name}' must create it"
            );
        }
    }
}

#[test]
fn every_rest_sidecar_sync_and_projection_reference_declared_names() {
    for path in sidecar_paths() {
        let file = name_of(&path);
        let cfg: IntegrationFileConfig =
            serde_yaml::from_str(&read(&path)).unwrap_or_else(|e| panic!("[{file}] parse: {e}"));
        let Some(rest) = cfg.transport.rest.as_ref() else {
            continue; // only the rest transport uses `calls`
        };

        // pagination bound is sane on every call.
        for (call_name, call) in &rest.calls {
            if let Some(pg) = &call.pagination {
                assert!(
                    pg.max_pages >= 1,
                    "[{file}] call '{call_name}' pagination.max_pages must be >= 1"
                );
            }
        }

        for (entity_name, entity) in &cfg.entities {
            let Some(sync) = entity.sync.as_ref() else {
                continue;
            };
            // A rest entity syncs via a named `calls` entry.
            if let Some(list_tool) = &sync.list_tool {
                assert!(
                    rest.calls.contains_key(list_tool),
                    "[{file}] entity '{entity_name}' sync.list_tool '{list_tool}' is not a \
                     declared transport.rest.calls entry"
                );
            }
            // Every projection target must be a declared schema column, else the
            // lifted value would land nowhere.
            let columns: Vec<&str> = entity.schema.iter().map(|f| f.name.as_str()).collect();
            for target in sync.project.keys() {
                assert!(
                    columns.contains(&target.as_str()),
                    "[{file}] entity '{entity_name}' projects into '{target}', which is not a \
                     declared schema column (have: {columns:?})"
                );
            }
        }
    }
}

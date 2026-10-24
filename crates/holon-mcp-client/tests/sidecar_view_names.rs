//! A sidecar `views:` name becomes a table name and an entity name, so it is
//! untrusted input the loader must parse: an unnameable view name is refused at
//! load with the field and the name, never carried on to registration.

use holon_mcp_client::McpSidecar;
use holon_mcp_client::mcp_integration::reconcile_sidecar_views;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

fn sidecar_yaml(view_name: &str) -> String {
    format!(
        "entity_prefix: \"cc_\"\n\
         entities:\n  \
           session:\n    \
             schema:\n      \
               - {{ name: id, sql_type: TEXT, primary_key: true }}\n      \
               - {{ name: ts, sql_type: TEXT }}\n\
         views:\n  \
           - name: {view_name:?}\n    \
             sql: \"SELECT id, MAX(ts) AS last_ts FROM cc_session GROUP BY id\"\n"
    )
}

async fn database_with_session_table() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("core schema");
    handle
        .execute_ddl("CREATE TABLE cc_session (id TEXT PRIMARY KEY, ts TEXT)")
        .await
        .expect("create the entity table");
    handle
}

async fn load_and_register(yaml: &str) -> anyhow::Result<DbHandle> {
    let sidecar = McpSidecar::from_yaml(yaml)?;
    let db = database_with_session_table().await;
    reconcile_sidecar_views(&sidecar, &db, "fixture").await?;
    Ok(db)
}

async fn view_exists(db: &DbHandle, name: &str) -> bool {
    !db.query(
        &format!("SELECT 1 FROM sqlite_schema WHERE type = 'view' AND name = '{name}'"),
        Default::default(),
    )
    .await
    .expect("read schema")
    .is_empty()
}

#[tokio::test]
async fn an_unnameable_view_name_is_refused_at_load() {
    let Err(err) = load_and_register(&sidecar_yaml("status now")).await else {
        panic!("a view name with a space names no table and no entity");
    };
    let msg = format!("{err:#}");
    for expected in ["`views`", "status now", "is not a valid entity name"] {
        assert!(msg.contains(expected), "`{expected}` missing from: {msg}");
    }
}

#[tokio::test]
async fn a_valid_view_name_registers_under_its_prefixed_table_name() {
    let db = load_and_register(&sidecar_yaml("session_status"))
        .await
        .expect("a valid view name loads and registers");
    assert!(view_exists(&db, "cc_session_status").await);
}

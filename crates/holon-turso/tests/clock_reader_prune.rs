//! `clock_reader` holds exactly the views that exist. A row for a dropped view
//! would make every later session tick its grain forever: a write, an IVM pass
//! and a CDC broadcast per minute for nothing.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::CoreSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn database_with_a_clock_reader() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    CoreSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("core schema");
    for ddl in [
        "CREATE TABLE msg (id TEXT PRIMARY KEY, ts TEXT)",
        "CREATE MATERIALIZED VIEW msg_clock AS SELECT id, ts, 'minute' AS clock_grain FROM msg",
        "CREATE MATERIALIZED VIEW msg_recent AS SELECT m.id, c.updated_at FROM msg_clock m JOIN \
         clock c ON c.grain = m.clock_grain",
        "CREATE MATERIALIZED VIEW msg_hourly AS SELECT m.id, c.today FROM msg_clock m JOIN clock \
         c ON c.grain = m.clock_grain",
    ] {
        handle.execute_ddl(ddl).await.expect("create");
    }
    for (view, grain) in [
        ("msg_recent", "minute"),
        ("msg_hourly", "hour"),
        ("msg_hourly", "minute"),
    ] {
        handle
            .execute(
                "INSERT INTO clock_reader (view, grain) VALUES (?, ?)",
                vec![
                    turso::Value::Text(view.to_string()),
                    turso::Value::Text(grain.to_string()),
                ],
            )
            .await
            .expect("record the reader");
    }
    handle
}

async fn readers(handle: &DbHandle) -> Vec<String> {
    handle
        .query(
            "SELECT view, grain FROM clock_reader ORDER BY view, grain",
            HashMap::new(),
        )
        .await
        .expect("read clock_reader")
        .iter()
        .map(|row| match (row.get("view"), row.get("grain")) {
            (Some(Value::String(view)), Some(Value::String(grain))) => format!("{view}/{grain}"),
            other => panic!("clock_reader row is {other:?}"),
        })
        .collect()
}

const SURVIVORS: [&str; 2] = ["msg_hourly/hour", "msg_hourly/minute"];

#[tokio::test]
async fn dropping_a_view_removes_its_clock_reader_row() {
    let handle = database_with_a_clock_reader().await;
    assert_eq!(
        readers(&handle).await,
        ["msg_hourly/hour", "msg_hourly/minute", "msg_recent/minute"]
    );
    handle
        .execute_ddl("DROP VIEW IF EXISTS msg_recent")
        .await
        .expect("drop the view");
    assert_eq!(
        readers(&handle).await,
        SURVIVORS,
        "the prune must remove exactly the dropped view's row: a kept row ticks its grain in every \
         later session, a lost one stops a live view's grain"
    );
}

#[tokio::test]
async fn replacing_a_view_removes_its_row_until_it_is_recorded_again() {
    let handle = database_with_a_clock_reader().await;
    let replaced = holon_turso::matview_manager::reconcile_named_view(
        &handle,
        "msg_recent",
        "SELECT id, ts FROM msg",
    )
    .await
    .expect("replace the view with one that reads no clock");
    assert!(replaced);
    assert_eq!(
        readers(&handle).await,
        SURVIVORS,
        "the prune must remove exactly the replaced view's row"
    );
}

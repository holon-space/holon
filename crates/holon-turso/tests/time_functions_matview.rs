//! A matview may apply a date and time function to a column: the value
//! depends only on the row, so the engine maintains it. A view that reads the
//! clock or the local timezone would go stale between statements, so the
//! engine refuses it at CREATE and the caller gets the error.

use std::collections::HashMap;

use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn setup() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
        .execute_ddl("CREATE TABLE ev (id INTEGER PRIMARY KEY, ts TEXT, until TEXT)")
        .await
        .expect("create table");
    handle
        .execute(
            "INSERT INTO ev (id, ts, until) VALUES (1, '2026-10-30 12:34:56', '2026-11-02 08:00:00')",
            vec![],
        )
        .await
        .expect("seed row");
    handle
}

async fn rows(handle: &DbHandle, sql: &str) -> Vec<String> {
    handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|r| format!("{:?}", r.get("d")))
        .collect()
}

#[tokio::test]
async fn time_functions_of_a_column_build_and_follow_later_rows() {
    let handle = setup().await;
    let exprs: &[(&str, &str)] = &[
        ("strftime", "strftime('%Y-%W', ts)"),
        ("date", "date(ts)"),
        ("time", "time(ts)"),
        ("datetime", "datetime(ts, '+1 day')"),
        ("julianday", "julianday(ts)"),
        ("unixepoch", "unixepoch(ts)"),
        ("timediff", "timediff(until, ts)"),
    ];
    for (name, expr) in exprs {
        let select = format!("SELECT id, {expr} AS d FROM ev");
        reconcile_named_view(&handle, &format!("tf_{name}"), &select)
            .await
            .unwrap_or_else(|e| panic!("{name}: `{expr}` over a column must build: {e}"));
    }
    handle
        .execute(
            "INSERT INTO ev (id, ts, until) VALUES (2, '2027-01-01 00:00:01', '2027-01-01 00:00:02')",
            vec![],
        )
        .await
        .expect("insert after the views exist");
    for (name, expr) in exprs {
        assert_eq!(
            rows(&handle, &format!("SELECT d FROM tf_{name} ORDER BY id")).await,
            rows(&handle, &format!("SELECT {expr} AS d FROM ev ORDER BY id")).await,
            "{name}: the view must hold what a plain query computes"
        );
    }
}

#[tokio::test]
async fn a_view_that_reads_the_clock_is_refused_at_create() {
    let handle = setup().await;
    let exprs: &[(&str, &str)] = &[
        ("now", "date('now')"),
        ("no_time_value", "date()"),
        ("localtime", "datetime(ts, 'localtime')"),
        ("current_timestamp", "CURRENT_TIMESTAMP"),
    ];
    for (name, expr) in exprs {
        let view = format!("clock_{name}");
        let err = reconcile_named_view(&handle, &view, &format!("SELECT id, {expr} AS d FROM ev"))
            .await
            .expect_err(&format!(
                "{name}: `{expr}` reads the clock and must be refused"
            ));
        assert!(
            format!("{err:#}").contains("non-deterministic"),
            "{name}: the refusal must say why; got {err:#}"
        );
        let left = handle
            .query(
                &format!("SELECT name FROM sqlite_schema WHERE name = '{view}'"),
                HashMap::new(),
            )
            .await
            .expect("read schema");
        assert!(left.is_empty(), "{name}: a refused view must not exist");
    }
}

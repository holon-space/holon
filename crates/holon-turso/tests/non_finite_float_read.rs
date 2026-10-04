//! Contract: a REAL that SQL computes as NaN or infinity never comes back as a
//! `Value`; every read path refuses it and names the column.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures::StreamExt;
use holon_turso::matview_manager::MatviewManager;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn setup() -> DbHandle {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(backend);
    handle
        .execute_ddl("CREATE TABLE m (id TEXT PRIMARY KEY, big REAL)")
        .await
        .expect("create table");
    handle
        .execute("INSERT INTO m (id, big) VALUES ('r0', 1.0)", vec![])
        .await
        .expect("seed row");
    handle
}

async fn insert(handle: &DbHandle, id: &str, big: f64) {
    handle
        .execute(
            "INSERT INTO m (id, big) VALUES (?, ?)",
            vec![turso::Value::Text(id.into()), turso::Value::Real(big)],
        )
        .await
        .expect("insert");
}

#[tokio::test]
async fn a_select_computing_a_non_finite_float_is_refused_naming_its_column() {
    let handle = setup().await;
    insert(&handle, "r1", 1.0e308).await;
    for sql in [
        "SELECT big * 10.0 AS x FROM m WHERE id = 'r1'",
        "SELECT 9.0e999 AS x",
    ] {
        let err = handle
            .query(sql, HashMap::new())
            .await
            .expect_err(&format!("{sql} computes inf"))
            .to_string();
        assert!(
            err.contains("'x' is the non-finite float inf"),
            "{sql}: {err}"
        );
    }
}

#[tokio::test]
async fn one_non_finite_row_fails_the_whole_read_of_a_view() {
    let handle = setup().await;
    let manager = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let view = manager
        .ensure_view("SELECT id, big * 10.0 AS d FROM m")
        .await
        .expect("create view");
    assert_eq!(
        manager.query_view(&view).await.expect("finite rows").len(),
        1
    );

    insert(&handle, "r1", 1.0e308).await;
    let err = manager
        .query_view(&view)
        .await
        .expect_err("the view now holds an infinite d for r1");
    let err = format!("{err:#}");
    assert!(err.contains("'d' is the non-finite float inf"), "{err}");
}

#[tokio::test]
async fn a_watched_view_discloses_a_row_that_computes_non_finite() {
    let handle = setup().await;
    let manager = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let (view, mut stream) = manager
        .ensure_and_subscribe("SELECT id, big * 10.0 AS d FROM m", None)
        .await
        .expect("subscribe");

    insert(&handle, "r1", 1.0e308).await;
    let batch = tokio::time::timeout(Duration::from_secs(5), stream.next())
        .await
        .unwrap_or_else(|_| panic!("the subscriber of {view} was never told"))
        .expect("the stream ended");
    assert!(
        batch.inner.items.is_empty(),
        "no row may carry the infinite d: {:?}",
        batch.inner.items
    );
    let note = batch
        .metadata
        .degraded
        .expect("the omitted row must be disclosed");
    assert!(note.contains("'d' is the non-finite float inf"), "{note}");
}

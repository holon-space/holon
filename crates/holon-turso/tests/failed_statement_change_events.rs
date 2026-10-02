//! A materialized view's change events describe exactly the rows a committed
//! transaction changed. A commit stamp minted per delivered event is fed only
//! when those rows reach the views, so an event for a row that never
//! committed would mint a stamp no feed can cover.
//!
//! The case: inside an explicit transaction a multi-row INSERT fails on a
//! duplicate key after its first row went in, and the transaction then
//! commits.

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::time::Duration;

use holon_api::BatchWithMetadata;
use holon_api::streaming::Change;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::RowChange;
use holon_turso::turso::TursoBackend;
use tokio::sync::broadcast::Receiver;

const VIEW: &str = "fsce_view";

async fn ids(handle: &DbHandle, sql: &str) -> BTreeSet<String> {
    handle
        .query(sql, HashMap::new())
        .await
        .unwrap_or_else(|e| panic!("{sql}: {e}"))
        .iter()
        .map(|r| {
            r.get("id")
                .and_then(|v| v.as_string())
                .expect("id text")
                .to_string()
        })
        .collect()
}

/// Every event the view delivered so far, as `C:<id>` / `U:<id>` / `D:<id>`.
async fn drain(rx: &mut Receiver<BatchWithMetadata<RowChange>>) -> Vec<String> {
    tokio::time::sleep(Duration::from_millis(250)).await;
    let mut out = Vec::new();
    while let Ok(batch) = rx.try_recv() {
        if batch.metadata.relation_name != VIEW {
            continue;
        }
        for item in &batch.inner.items {
            out.push(match &item.change {
                Change::Created { data, .. } => {
                    format!("C:{}", data.get("id").and_then(|v| v.as_string()).unwrap())
                }
                Change::Updated { id, .. } => format!("U:{id}"),
                Change::Deleted { id, .. } => format!("D:{id}"),
                Change::FieldsChanged { entity_id, .. } => format!("F:{entity_id}"),
            });
        }
    }
    out
}

fn text(s: &str) -> turso::Value {
    turso::Value::Text(s.into())
}

#[tokio::test]
async fn a_failed_statement_in_a_committed_transaction_leaves_no_change_event() {
    let (backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    let mut rx = handle.subscribe_row_changes();
    handle
        .execute(
            "CREATE TABLE fsce_base (id TEXT PRIMARY KEY, v TEXT)",
            vec![],
        )
        .await
        .expect("create base");
    handle
        .execute(
            "INSERT INTO fsce_base (id, v) VALUES (?, ?)",
            vec![text("dup"), text("seed")],
        )
        .await
        .expect("seed row");
    reconcile_named_view(&handle, VIEW, "SELECT id, v FROM fsce_base")
        .await
        .expect("matview");
    let before = ids(&handle, "SELECT id FROM fsce_base").await;
    let _ = drain(&mut rx).await;

    handle.execute("BEGIN", vec![]).await.expect("BEGIN");
    handle
        .execute(
            "INSERT INTO fsce_base (id, v) VALUES (?, ?)",
            vec![text("good"), text("1")],
        )
        .await
        .expect("good insert");
    let failed = handle
        .execute(
            "INSERT INTO fsce_base (id, v) VALUES (?, ?), (?, ?)",
            vec![text("partial"), text("2"), text("dup"), text("3")],
        )
        .await;
    assert!(
        failed.is_err(),
        "the duplicate key must fail the statement, got {failed:?}"
    );
    let commit = handle.execute("COMMIT", vec![]).await;
    eprintln!("COMMIT after the failed statement: {commit:?}");

    let after = ids(&handle, "SELECT id FROM fsce_base").await;
    let view = ids(&handle, &format!("SELECT id FROM {VIEW}")).await;
    let events = drain(&mut rx).await;
    let committed: Vec<String> = after
        .difference(&before)
        .map(|id| format!("C:{id}"))
        .collect();
    eprintln!("base after: {after:?}\nview after: {view:?}\nview events: {events:?}");

    assert_eq!(view, after, "the view's rows differ from its base table");
    assert_eq!(
        events, committed,
        "the view's change events differ from the rows the transaction committed"
    );
    drop(backend);
}

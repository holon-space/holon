//! C4 seat A — a SQL-compilable derived field, PLANTED as a matview column,
//! is maintained by Turso IVM through base-row writes.
//!
//! This is the end-to-end proof for the hybrid seat's IVM half: a `Computation`
//! that lowers to SQL is classified by `DerivedFieldPlan` into a
//! `PlantedColumn`, that column's inlined expression goes into a `CREATE
//! MATERIALIZED VIEW`, and IVM recomputes/RETRACTS it O(delta) as the input
//! column changes — no projection-stage evaluation, no manual recompute. The
//! Rhai-only (seat B) half and the retraction contract for it are proven by the
//! unit tests in `holon-api/src/computation.rs`.

use std::collections::HashMap;

use holon_api::Value;
use holon_api::computation::ArithOp;
use holon_api::computation::Computation;
use holon_api::computation::DerivedField;
use holon_api::computation::DerivedFieldPlan;
use holon_api::computation::FieldIdent;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend); // keep the actor alive for the test
    handle
        .execute_ddl("CREATE TABLE task (id TEXT PRIMARY KEY, priority INTEGER)")
        .await
        .expect("create base table");
    handle
}

async fn set_priority(handle: &DbHandle, id: &str, priority: i64) {
    handle
        .execute(
            "INSERT INTO task (id, priority) VALUES (?, ?) ON CONFLICT(id) DO UPDATE SET priority \
             = excluded.priority",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Integer(priority),
            ],
        )
        .await
        .expect("upsert task");
}

async fn delete_task(handle: &DbHandle, id: &str) {
    handle
        .execute(
            "DELETE FROM task WHERE id = ?",
            vec![turso::Value::Text(id.into())],
        )
        .await
        .expect("delete task");
}

/// Read `(id, boosted)` from the derived-field matview.
async fn read_boosted(handle: &DbHandle) -> Vec<(String, i64)> {
    let rows = handle
        .query(
            "SELECT id, boosted FROM task_derived ORDER BY id",
            HashMap::new(),
        )
        .await
        .expect("query derived view");
    let mut out: Vec<(String, i64)> = rows
        .iter()
        .map(|r| {
            let id = match r.get("id") {
                Some(Value::String(s)) => s.clone(),
                other => panic!("id: unexpected {other:?}"),
            };
            let boosted = match r.get("boosted") {
                Some(Value::Integer(i)) => *i,
                Some(Value::Float(f)) => *f as i64,
                other => panic!("boosted: unexpected {other:?}"),
            };
            (id, boosted)
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn planted_computed_column_is_ivm_maintained_and_retracts() {
    let handle = setup().await;

    // Declaration -> Computation -> plan. `priority * 2` lowers to SQL, so the
    // hybrid seat routes it to seat A (a planted matview column), NOT the stage.
    let plan = DerivedFieldPlan::plan(vec![DerivedField::new(
        FieldIdent::parse("boosted").expect("identifier"),
        Computation::Arith {
            op: ArithOp::Mul,
            lhs: Box::new(Computation::Field("priority".into())),
            rhs: Box::new(Computation::Lit(Value::Integer(2))),
        },
    )]);
    assert_eq!(plan.sql_planted.len(), 1, "field must be SQL-planted");
    assert!(
        plan.stage_evaluated.is_empty(),
        "nothing falls to the stage"
    );
    let col = &plan.sql_planted[0];
    assert_eq!(col.sql, "(priority * 2)");
    assert_eq!(col.select_expr(), "((priority * 2)) AS \"boosted\"");

    // Plant the column into a matview over the base table.
    let select = format!("SELECT id, {} FROM task", col.select_expr());
    let created = reconcile_named_view(&handle, "task_derived", &select)
        .await
        .expect("planted matview DDL must succeed");
    assert!(created, "first reconcile creates the view");

    // Seed + initial maintenance.
    set_priority(&handle, "t1", 3).await;
    set_priority(&handle, "t2", 1).await;
    assert_eq!(
        read_boosted(&handle).await,
        vec![("t1".to_string(), 6), ("t2".to_string(), 2)]
    );

    // INPUT CHANGE: IVM recomputes only the affected row — the old derived value
    // is replaced, never stacked.
    set_priority(&handle, "t1", 5).await;
    assert_eq!(
        read_boosted(&handle).await,
        vec![("t1".to_string(), 10), ("t2".to_string(), 2)]
    );

    // RETRACTION: deleting the base row drops the derived row.
    delete_task(&handle, "t1").await;
    assert_eq!(read_boosted(&handle).await, vec![("t2".to_string(), 2)]);
}

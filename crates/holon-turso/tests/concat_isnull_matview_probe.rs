//! I3-0 / R5 PROBE: does the Turso fork's IVM matview planner accept the SQL
//! shapes `Computation::Concat` / `And` / `IsDefined` lower to?
//!
//! The planner is already known to reject `CASE` at DDL (hence `iif`), so
//! `||`, `IS NOT NULL`, `AND` and `NULL` literals cannot be assumed. If a shape
//! is rejected here, computations using it must classify `stage_evaluated` via
//! the existing `DerivedFieldPlan` fallback rather than being planted.

use std::collections::HashMap;

use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl("CREATE TABLE p (id TEXT PRIMARY KEY, role TEXT, email TEXT, n INTEGER)")
        .await
        .expect("create table");
    handle
        .execute(
            "INSERT INTO p (id, role, email, n) VALUES ('r1', 'CTO', 'm@x', 7), ('r2', NULL, \
             'a@b', 3)",
            vec![],
        )
        .await
        .expect("seed rows");
    handle
}

#[tokio::test]
async fn probe_matview_accepts_concat_and_isnull_shapes() {
    let handle = setup().await;

    let shapes: &[(&str, &str)] = &[
        ("concat", "(role || ' — ' || email)"),
        ("is_not_null", "(role IS NOT NULL)"),
        ("is_null", "(role IS NULL)"),
        (
            "logical_and",
            "((role IS NOT NULL) AND (email IS NOT NULL))",
        ),
        ("null_literal", "(role IS NOT NULL AND NULL IS NULL)"),
        (
            "acceptance_shape",
            "iif(1 = ((role IS NOT NULL) AND (role IS NOT NULL)), ((role || ' — ') || email), \
             email)",
        ),
        ("concat_int_coercion", "('n=' || n)"),
        ("coalesce", "COALESCE(role, email)"),
    ];

    let mut verdicts = Vec::new();
    for (name, expr) in shapes {
        let view = format!("probe_{name}");
        let select = format!("SELECT id, {expr} AS d FROM p");
        match reconcile_named_view(&handle, &view, &select).await {
            Ok(_) => {
                let rows = handle
                    .query(
                        &format!("SELECT id, d FROM {view} ORDER BY id"),
                        HashMap::new(),
                    )
                    .await
                    .expect("query probe view");
                let vals: Vec<String> = rows.iter().map(|r| format!("{:?}", r.get("d"))).collect();
                verdicts.push(format!("ACCEPT {name:24} {expr}  ->  {vals:?}"));
            }
            Err(e) => verdicts.push(format!("REJECT {name:24} {expr}  ->  {e}")),
        }
    }

    for v in &verdicts {
        println!("{v}");
    }
    // The probe records; it never fails. Its output is the R5 evidence.
}

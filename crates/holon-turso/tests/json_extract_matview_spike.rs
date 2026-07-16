//! C4 A2 SPIKE — the biggest unverified assumption of the derived-fields
//! workstream: which derived-column shapes can the fork's IVM maintain in a
//! materialized view, O(delta), through base-row writes and deletes?
//!
//! Two axes matter for the binding + `Computation::Case` lowering choices:
//!   1. `json_extract(properties, '$.p')` — block properties live in the
//!      `block_raw` JSON `properties` column, so json_extract Field-binding
//!      depends on the IVM maintaining it.
//!   2. conditional lowering — `Computation::Case` can lower to either a
//!      searched `CASE WHEN` or nested `iif(...)`. `sidecar_views.rs` already
//!      ships `iif` in a chained view; this pins down what a NON-chained
//!      derived matview accepts.
//!
//! Each probe is an independent test so the support matrix is legible. A plain
//! table with a JSON column mirrors `block_raw`.
//!
//! VERDICT (this file, green, is the executable record):
//!   * json_extract as a BARE derived column  -> IVM-maintained + retracts  ✅
//!   * `iif(cond, a, b)` (incl. over json_extract) -> IVM-maintained         ✅
//!   * searched `CASE WHEN …` and simple `CASE x WHEN …` -> REJECTED at DDL
//!     ("Cannot convert LogicalExpr to AST Expr: Case { … }") by the fork's
//!     matview AST conversion — NOT plantable.                               ❌
//!
//! Consequences for the workstream:
//!   1. json_extract Field-binding is UNBLOCKED (bare extraction plants).
//!   2. `Computation::Case::compile_sql` lowers to nested `iif(...)`, NEVER to
//!      SQL `CASE WHEN`, because CASE does not survive the fork's IVM planning.

use std::collections::HashMap;

use holon_api::Value;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl("CREATE TABLE blk (id TEXT PRIMARY KEY, priority INTEGER, properties TEXT)")
        .await
        .expect("create base table");
    handle
}

async fn set_row(handle: &DbHandle, id: &str, priority: i64) {
    let props = format!("{{\"p\": {priority}}}");
    handle
        .execute(
            "INSERT INTO blk (id, priority, properties) VALUES (?, ?, ?) ON CONFLICT(id) DO \
             UPDATE SET priority = excluded.priority, properties = excluded.properties",
            vec![
                turso::Value::Text(id.into()),
                turso::Value::Integer(priority),
                turso::Value::Text(props),
            ],
        )
        .await
        .expect("upsert blk");
}

async fn delete_row(handle: &DbHandle, id: &str) {
    handle
        .execute(
            "DELETE FROM blk WHERE id = ?",
            vec![turso::Value::Text(id.into())],
        )
        .await
        .expect("delete blk");
}

/// Read `(id, d)` from a single-derived-column view named `view`.
async fn read_d(handle: &DbHandle, view: &str) -> Vec<(String, i64)> {
    let rows = handle
        .query(
            &format!("SELECT id, d FROM {view} ORDER BY id"),
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
            let d = match r.get("d") {
                Some(Value::Integer(i)) => *i,
                Some(Value::Float(f)) => *f as i64,
                Some(Value::String(s)) => s.parse::<i64>().expect("numeric text"),
                Some(Value::Null) => 0,
                other => panic!("d: unexpected {other:?}"),
            };
            (id, d)
        })
        .collect();
    out.sort();
    out
}

/// Drive create → seed → mutate → delete against a single-column derived view
/// and assert O(delta) maintenance + retraction. `expr` is the `d` column.
/// `d_of` computes the expected derived value for a given priority.
async fn assert_maintained(view: &str, expr: &str, d_of: impl Fn(i64) -> i64) {
    let handle = setup().await;
    let select = format!("SELECT id, {expr} AS d FROM blk");
    reconcile_named_view(&handle, view, &select)
        .await
        .unwrap_or_else(|e| panic!("DDL for `{expr}` must succeed: {e}"));

    set_row(&handle, "b1", 3).await;
    set_row(&handle, "b2", 8).await;
    assert_eq!(
        read_d(&handle, view).await,
        vec![("b1".into(), d_of(3)), ("b2".into(), d_of(8))],
        "initial maintenance for `{expr}`"
    );

    set_row(&handle, "b1", 9).await; // mutate across any threshold
    assert_eq!(
        read_d(&handle, view).await,
        vec![("b1".into(), d_of(9)), ("b2".into(), d_of(8))],
        "mutation maintenance for `{expr}`"
    );

    delete_row(&handle, "b1").await;
    assert_eq!(
        read_d(&handle, view).await,
        vec![("b2".into(), d_of(8))],
        "retraction for `{expr}`"
    );
}

// --- Axis 1: json_extract as a bare derived column ------------------------

#[tokio::test]
async fn probe_bare_json_extract_column() {
    assert_maintained("v_bare_json", "json_extract(properties, '$.p')", |p| p).await;
}

/// Assert the fork's IVM matview planner REJECTS `expr` at DDL time with the
/// `Case` LogicalExpr-conversion error. This pins the limitation that forces
/// `iif` lowering — if a future fork bump makes CASE plantable, this test flips
/// RED and we revisit the lowering.
async fn assert_ddl_rejects_case(view: &str, expr: &str) {
    let handle = setup().await;
    let select = format!("SELECT id, {expr} AS d FROM blk");
    let err = reconcile_named_view(&handle, view, &select)
        .await
        .expect_err("fork IVM must reject CASE in a matview SELECT");
    let msg = err.to_string();
    assert!(
        msg.contains("Cannot convert LogicalExpr to AST Expr: Case"),
        "expected the CASE conversion rejection, got: {msg}"
    );
}

// --- Axis 2: conditional lowering over a PLAIN column ----------------------
// CASE (searched AND simple) is REJECTED; iif is accepted.

#[tokio::test]
async fn probe_searched_case_rejected() {
    assert_ddl_rejects_case("v_case_plain", "CASE WHEN priority > 5 THEN 1 ELSE 0 END").await;
}

#[tokio::test]
async fn probe_simple_case_rejected() {
    assert_ddl_rejects_case(
        "v_simplecase_plain",
        "CASE priority WHEN 3 THEN 100 WHEN 8 THEN 40 ELSE 1 END",
    )
    .await;
}

#[tokio::test]
async fn probe_iif_over_plain_column() {
    assert_maintained("v_iif_plain", "iif(priority > 5, 1, 0)", |p| (p > 5) as i64).await;
}

// --- Axis 1 x 2: conditional lowering over json_extract --------------------

#[tokio::test]
async fn probe_searched_case_over_json_extract_rejected() {
    assert_ddl_rejects_case(
        "v_case_json",
        "CASE WHEN json_extract(properties, '$.p') > 5 THEN 1 ELSE 0 END",
    )
    .await;
}

#[tokio::test]
async fn probe_iif_over_json_extract() {
    assert_maintained(
        "v_iif_json",
        "iif(json_extract(properties, '$.p') > 5, 1, 0)",
        |p| (p > 5) as i64,
    )
    .await;
}

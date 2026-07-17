//! C4 A2 — THIRD-EVALUATOR agreement: `Computation::eval` (in-memory) vs the
//! SAME computation PLANTED as a real matview column and evaluated by SQLite.
//!
//! The dual-eval PBT (holon-api side) proves eval == Rhai. This closes the
//! triangle toward eval == SQL — the leg the verifier found untested.
//!
//! ⚠️ DISCOVERED FORK BUG (pinned below, flagged to the orchestrator): the
//! Turso fork's IVM **matview** logical plan drops REAL affinity from
//! whole-number float literals (`3.0` is planned as integer `3`), so `(3.0 /
//! 2.0)` maintains as `1` and `(xi / 2.0)` as `4` inside a matview — although
//! the direct query engine and any genuine REAL column (`xf`) are correct. This
//! is a fork bug, NOT a rendering bug (inline_sql correctly emits `3.0`;
//! verified in holon-api). A2 only *computes* the seat routing; it does not yet
//! plant live derived matviews (that is the sidecar increment), so this does
//! not regress production — it de-risks the next increment.
//! `planted_sql_matches_eval` asserts agreement over the fork-CORRECT cases;
//! `matview_whole_float_literal_ bug_is_pinned` records the fork bug as an
//! executable, loud known-divergence that flips RED the day the fork is fixed.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::Value;
use holon_api::computation::ArithOp;
use holon_api::computation::Computation;
use holon_api::computation::DerivedField;
use holon_api::computation::DerivedFieldPlan;
use holon_turso::derived_reconciler::spawn_derived_field_reconciler;
use holon_turso::matview_manager::MatviewManager;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockDerivedSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;

async fn setup() -> DbHandle {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl("CREATE TABLE t (id TEXT PRIMARY KEY, xi INTEGER, xf REAL, props TEXT)")
        .await
        .expect("create table");
    // One fixed row. `xi` is a genuine INTEGER column, `xf` a REAL column.
    handle
        .execute(
            "INSERT INTO t (id, xi, xf, props) VALUES ('r1', 9, 5.0, '{\"n\": 7}')",
            vec![],
        )
        .await
        .expect("seed row");
    handle
}

/// Plant `comp` as a matview column via `DerivedFieldPlan::plan`, then read the
/// single derived value SQLite computed.
async fn plant_and_read(handle: &DbHandle, view: &str, comp: Computation) -> Value {
    let plan = DerivedFieldPlan::plan(vec![DerivedField::new("d", comp)]);
    assert_eq!(plan.sql_planted.len(), 1, "expr must plant (seat A)");
    assert!(
        plan.stage_evaluated.is_empty(),
        "expr must not fall to stage"
    );
    let col = &plan.sql_planted[0];
    let select = format!("SELECT id, {} AS d FROM t", col.sql);
    reconcile_named_view(handle, view, &select)
        .await
        .unwrap_or_else(|e| panic!("planted DDL `{}` must succeed: {e}", col.sql));
    let rows = handle
        .query(&format!("SELECT d FROM {view}"), HashMap::new())
        .await
        .expect("query planted view");
    rows[0].get("d").cloned().expect("d column present")
}

fn approx_eq(a: &Value, b: &Value) -> bool {
    match (a.as_f64(), b.as_f64()) {
        (Some(x), Some(y)) => (x - y).abs() <= 1e-9 * (1.0 + x.abs().max(y.abs())),
        _ => a == b,
    }
}

fn ilit(i: i64) -> Box<Computation> {
    Box::new(Computation::Lit(Value::Integer(i)))
}
fn flit(f: f64) -> Box<Computation> {
    Box::new(Computation::Lit(Value::Float(f)))
}
fn field(n: &str) -> Box<Computation> {
    Box::new(Computation::Field(n.into()))
}
fn div(l: Box<Computation>, r: Box<Computation>) -> Computation {
    Computation::Arith {
        op: ArithOp::Div,
        lhs: l,
        rhs: r,
    }
}

#[tokio::test]
async fn planted_sql_matches_eval() {
    let handle = setup().await;
    // Row context matching the seeded row, for the in-memory eval side.
    let ctx: HashMap<String, Value> = HashMap::from([
        ("xi".to_string(), Value::Integer(9)),
        ("xf".to_string(), Value::Float(5.0)),
    ]);

    // Cases where the fork's matview IVM is CORRECT — eval must equal SQL.
    let cases: Vec<(&str, Computation)> = vec![
        // Refutation 1: integer division. eval = Integer(2), SQL `(5 / 2)` = 2.
        ("v_intdiv", div(ilit(5), ilit(2))),
        // Refutation 1b: `9 / 4 + 1` = 2 + 1 = 3 (integer division first).
        (
            "v_intdiv2",
            Computation::Arith {
                op: ArithOp::Add,
                lhs: Box::new(div(ilit(9), ilit(4))),
                rhs: ilit(1),
            },
        ),
        // Integer-column / integer-literal → integer division (9 / 2 = 4).
        ("v_intcol", div(field("xi"), ilit(2))),
        // REAL column / whole-float literal → the real column keeps affinity, so
        // the fork gets this right: 5.0 / 2.0 = 2.5.
        ("v_realcol", div(field("xf"), flit(2.0))),
        // Fractional float literal keeps REAL affinity: 0.001 * 9 = 0.009.
        (
            "v_frac",
            Computation::Arith {
                op: ArithOp::Mul,
                lhs: flit(0.001),
                rhs: field("xi"),
            },
        ),
    ];

    for (view, comp) in cases {
        let expected = comp.eval(&ctx).expect("eval must succeed");
        let sql_val = plant_and_read(&handle, view, comp).await;
        assert!(
            approx_eq(&expected, &sql_val),
            "{view}: eval={expected:?} SQL={sql_val:?}"
        );
    }
}

/// PINNED FORK BUG (loud, executable record — see the module header). The Turso
/// fork's matview logical plan integer-types whole-number float literals, so a
/// planted `(3.0 / 2.0)` maintains as `1` and `(xi / 2.0)` as `4`, diverging
/// from `Computation::eval` (1.5 and 4.5). Asserting the WRONG values here
/// means this test flips RED the moment the fork is fixed — the signal to
/// delete this test and fold these cases back into `planted_sql_matches_eval`.
/// Flagged to the orchestrator; the sidecar increment (which actually plants)
/// must not depend on whole-float-literal arithmetic over integer inputs until
/// fixed.
#[tokio::test]
async fn matview_whole_float_literal_bug_is_pinned() {
    let handle = setup().await;
    let ctx: HashMap<String, Value> = HashMap::from([("xi".to_string(), Value::Integer(9))]);

    // eval is CORRECT (type-faithful): 3.0 / 2.0 = 1.5.
    let whole = div(flit(3.0), flit(2.0));
    assert_eq!(whole.eval(&ctx).unwrap(), Value::Float(1.5));
    // ... but the fork's matview mis-types the literals and integer-divides.
    let sql = plant_and_read(&handle, "v_bug_litlit", whole).await;
    assert_eq!(
        sql,
        Value::Integer(1),
        "if this is no longer Integer(1), the fork bug is FIXED — delete this test"
    );

    // int column / whole-float literal: eval promotes to float (4.5), fork does
    // integer division (4).
    let mixed = div(field("xi"), flit(2.0));
    assert_eq!(mixed.eval(&ctx).unwrap(), Value::Float(4.5));
    let sql = plant_and_read(&handle, "v_bug_intcol", mixed).await;
    assert_eq!(
        sql,
        Value::Integer(4),
        "fork bug: int-col / whole-float-lit"
    );
}

/// THIRD LEG persisted: the value the SIDECAR watcher lands in `block_derived`
/// must equal BOTH `Computation::eval` and the SQL-planted matview value. The
/// watcher evaluates in Rust (so sidecar == eval holds by construction); this
/// test makes the full triangle explicit end-to-end, including the persisted
/// round-trip through `block_derived.value_json`.
///
/// Fractional literal (`* 1.5`) keeps REAL affinity, so the planted leg is
/// fork-CORRECT. TODO(pin-bump): once `fix/matview-whole-float-affinity` is
/// pinned, add a whole-float case here (its planted leg is wrong today — see
/// `matview_whole_float_literal_bug_is_pinned`).
#[tokio::test]
async fn sidecar_value_matches_eval_and_planted_sql() {
    let handle = setup().await;
    BlockDerivedSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block_derived table");
    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));

    // `d = xf * 1.5` over the seeded row (xf = 5.0) → 7.5. Plantable (seat A)
    // AND fractional-literal (fork-correct).
    let comp = Computation::Arith {
        op: ArithOp::Mul,
        lhs: field("xf"),
        rhs: flit(1.5),
    };

    // Leg 1 — eval.
    let ctx: HashMap<String, Value> = HashMap::from([("xf".to_string(), Value::Float(5.0))]);
    let eval_val = comp.eval(&ctx).expect("eval");

    // Leg 2 — planted SQL matview column.
    let planted_val = plant_and_read(&handle, "v_sidecar_planted", comp.clone()).await;

    // Leg 3 — sidecar, maintained by the CDC watcher.
    let _guard = spawn_derived_field_reconciler(
        &mgr,
        handle.clone(),
        "SELECT id, xf FROM t",
        vec![DerivedField::new("d", comp)],
    )
    .await
    .expect("spawn reconciler");

    let sidecar_val = await_sidecar(&handle, "r1", "d").await;

    assert!(
        approx_eq(&sidecar_val, &eval_val),
        "sidecar={sidecar_val:?} eval={eval_val:?}"
    );
    assert!(
        approx_eq(&sidecar_val, &planted_val),
        "sidecar={sidecar_val:?} planted={planted_val:?}"
    );
}

/// Poll `block_derived` for one field's persisted value, decoding the stored
/// JSON back into a [`Value`].
async fn await_sidecar(handle: &DbHandle, block_id: &str, field: &str) -> Value {
    for _ in 0..100 {
        let rows = handle
            .query_positional(
                "SELECT value_json FROM block_derived WHERE block_id = ? AND field_name = ?",
                vec![
                    turso::Value::Text(block_id.into()),
                    turso::Value::Text(field.into()),
                ],
            )
            .await
            .expect("query block_derived");
        if let Some(row) = rows.first() {
            let json = match row.get("value_json") {
                Some(Value::String(s)) => s.clone(),
                other => panic!("value_json: unexpected {other:?}"),
            };
            return serde_json::from_str::<Value>(&json).expect("decode sidecar value_json");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("sidecar {block_id}.{field} never appeared");
}

/// DOCUMENTED DIVERGENCE (not silent): an absent field.
///
/// `Computation::eval` on a missing field **fails loud** (`MissingField`),
/// whereas `json_extract(props, '$.absent')` yields SQL NULL. The A2 increment
/// does NOT yet lower `Field` to `json_extract` (that is the next, sidecar
/// increment), so today's planted `Field` is a bare column reference and this
/// gap cannot be reached in production. This test pins BOTH behaviours so the
/// reconciliation obligation is visible, not silent, when json_extract binding
/// lands. See docs/Plans/DERIVED-FIELDS-A2.md.
#[tokio::test]
async fn absent_field_divergence_is_documented() {
    let handle = setup().await;

    // eval side: missing field is loud.
    let err = Computation::Field("absent".into())
        .eval(&HashMap::new())
        .expect_err("eval must fail loud on a missing field");
    assert!(format!("{err}").contains("absent"));

    // SQL side: json_extract of an absent key is NULL (the future Field binding).
    reconcile_named_view(
        &handle,
        "v_absent",
        "SELECT id, json_extract(props, '$.absent') AS d FROM t",
    )
    .await
    .expect("json_extract view DDL");
    let rows = handle
        .query("SELECT d FROM v_absent", HashMap::new())
        .await
        .expect("query");
    assert_eq!(
        rows[0].get("d"),
        Some(&Value::Null),
        "json_extract of an absent key must be SQL NULL"
    );
}

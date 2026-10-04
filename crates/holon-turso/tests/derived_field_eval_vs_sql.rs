//! C4 A2 — THIRD-EVALUATOR agreement: `Computation::eval` (in-memory) vs the
//! SAME computation PLANTED as a real matview column and evaluated by SQLite.
//!
//! The dual-eval PBT (holon-api side) proves eval == Rhai. This closes the
//! triangle toward eval == SQL — the leg the verifier found untested.
//!
//! FIXED on the Turso v0.8 line (nightscape@holon 3ef4bece): the fork's IVM
//! **matview** logical plan previously dropped REAL affinity from whole-number
//! float literals (`3.0` was planned as integer `3`), so `(3.0 / 2.0)`
//! maintained as `1` and `(xi / 2.0)` as `4` inside a matview. v0.8 keeps REAL
//! affinity, matching `Computation::eval` and the direct query engine.
//! `planted_sql_matches_eval` asserts agreement over the always-correct cases;
//! `matview_whole_float_literal_affinity_matches_eval` (was the pinned-bug
//! test) now guards the fix — it went RED on the v0.7 -> v0.8 pin bump exactly
//! as its old contract promised, and was flipped to assert the correct values.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use holon_api::ConditionBus;
use holon_api::Value;
use holon_api::computation::ArithOp;
use holon_api::computation::Computation;
use holon_api::computation::ComputeError;
use holon_api::computation::DerivedField;
use holon_api::computation::DerivedFieldPlan;
use holon_api::computation::FieldKind;
use holon_api::computation::FieldTypes;
use holon_turso::derived_reconciler::spawn_derived_field_reconciler;
use holon_turso::matview_manager::MatviewManager;
use holon_turso::matview_manager::reconcile_named_view;
use holon_turso::schema_module::SchemaModule;
use holon_turso::schema_modules::BlockDerivedSchemaModule;
use holon_turso::turso::DbHandle;
use holon_turso::turso::TursoBackend;
use proptest::prelude::*;

fn fid(name: &str) -> holon_api::computation::FieldIdent {
    holon_api::computation::FieldIdent::parse(name).expect("test identifier")
}

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
    let plan = DerivedFieldPlan::plan(vec![DerivedField::new(fid("d"), comp)]);
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

// ---------------------------------------------------------------------------
// I3-0: `Concat` / `And` / `IsDefined` / the unit literal, against real SQLite.
// ---------------------------------------------------------------------------

/// Real SQLite is the oracle for the semantics `Computation::eval` had to pick
/// for [`Computation::Concat`]: NULL propagation and numeric-to-text coercion.
/// `eval` chose SQL-faithfully rather than Rhai-faithfully precisely because
/// this planted column is what production reads — so this test, not the
/// Rhai-differential PBT, is what decides those two rules are right.
#[tokio::test]
async fn concat_and_definedness_match_sqlite() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl(
            "CREATE TABLE q (id TEXT PRIMARY KEY, role TEXT, email TEXT, n INTEGER, f REAL, fw REAL)",
        )
        .await
        .expect("create table");
    handle
        .execute(
            "INSERT INTO q (id, role, email, n, f, fw) VALUES ('r1', 'CTO', 'ada@x', 1, 2.5, 1.0)",
            vec![],
        )
        .await
        .expect("seed row");

    let ctx: HashMap<String, Value> = HashMap::from([
        ("role".to_string(), Value::String("CTO".into())),
        ("email".to_string(), Value::String("ada@x".into())),
        ("n".to_string(), Value::Integer(1)),
        ("f".to_string(), Value::Float(2.5)),
        ("fw".to_string(), Value::Float(1.0)),
    ]);

    let sources = [
        // Text join, multibyte separator.
        r#"role + " — " + email"#,
        // Numeric-to-text coercion, both kinds.
        r#""n=" + n"#,
        r#""f=" + f"#,
        // Whole float: `{:?}` keeps the decimal point, matching SQLite's REAL text.
        r#""fw=" + fw"#,
        // Definedness and the unit comparison, in isolation and conjoined.
        r#"if is_def_var("role") { role } else { email }"#,
        r#"if role != () { role } else { email }"#,
        // The acceptance case itself.
        r#"if is_def_var("role") && role != () { role + " — " + email } else { email }"#,
    ];

    for (i, src) in sources.iter().enumerate() {
        let comp = holon_api::expr_parser::parse(src)
            .unwrap_or_else(|e| panic!("subset must parse `{src}`: {e}"));
        let expected = comp
            .eval(&ctx)
            .unwrap_or_else(|e| panic!("`{src}` must evaluate: {e}"));

        let plan = DerivedFieldPlan::plan(vec![DerivedField::new(fid("d"), comp)]);
        assert_eq!(
            plan.sql_planted.len(),
            1,
            "`{src}` must plant (seat A); stage={:?}",
            plan.stage_evaluated
        );
        let col = &plan.sql_planted[0].sql;
        let view = format!("v_concat_{i}");
        reconcile_named_view(&handle, &view, &format!("SELECT id, {col} AS d FROM q"))
            .await
            .unwrap_or_else(|e| panic!("planted DDL `{col}` for `{src}` must succeed: {e}"));
        let rows = handle
            .query(&format!("SELECT d FROM {view}"), HashMap::new())
            .await
            .expect("query planted view");
        let sql_val = rows[0].get("d").cloned().expect("d column");
        assert_eq!(
            expected, sql_val,
            "`{src}`: eval={expected:?} matview_sql={sql_val:?} (planted `{col}`)"
        );
    }
}

/// NULL propagation through `||`, on a row where the guarded column IS null —
/// the state the whole `is_def_var(x) && x != ()` idiom exists to handle, and
/// the one a row can actually be in.
#[tokio::test]
async fn null_role_takes_the_else_branch_in_both_engines() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl("CREATE TABLE qn (id TEXT PRIMARY KEY, role TEXT, email TEXT)")
        .await
        .expect("create table");
    handle
        .execute(
            "INSERT INTO qn (id, role, email) VALUES ('r1', NULL, 'ada@x')",
            vec![],
        )
        .await
        .expect("seed row");

    let src = r#"if is_def_var("role") && role != () { role + " — " + email } else { email }"#;
    let comp = holon_api::expr_parser::parse(src).expect("must parse");
    // In-memory, a NULL column reads back as `Value::Null`.
    let ctx: HashMap<String, Value> = HashMap::from([
        ("role".to_string(), Value::Null),
        ("email".to_string(), Value::String("ada@x".into())),
    ]);
    let expected = comp.eval(&ctx).expect("must evaluate");
    assert_eq!(expected, Value::String("ada@x".to_string()));

    let plan = DerivedFieldPlan::plan(vec![DerivedField::new(fid("d"), comp)]);
    let col = &plan.sql_planted[0].sql;
    reconcile_named_view(
        &handle,
        "v_nullrole",
        &format!("SELECT id, {col} AS d FROM qn"),
    )
    .await
    .unwrap_or_else(|e| panic!("planted DDL `{col}` must succeed: {e}"));
    let rows = handle
        .query("SELECT d FROM v_nullrole", HashMap::new())
        .await
        .expect("query planted view");
    let sql_val = rows[0].get("d").cloned().expect("d column");
    assert_eq!(expected, sql_val, "planted `{col}`");
}

/// THE ONE STATE WHERE `IsDefined` DISAGREES ACROSS THE SEATS, pinned rather
/// than fixed — the divergence `Computation::IsDefined`'s doc comment names.
///
/// `eval` is key-presence (matching Rhai: a key bound to `Value::Null` IS
/// defined); the lowering `(role IS NOT NULL)` is false on the equivalent row,
/// because a row has no spelling for "absent" other than NULL. Isolated, the
/// two therefore disagree. Conjoined with `role != ()` — the idiom the shape
/// exists to serve — they agree on all three states, which is what
/// `null_role_takes_the_else_branch_in_both_engines` shows.
///
/// A fix would have to invent an absence marker inside the row world; pinning
/// keeps the disagreement visible instead of letting it be rediscovered.
#[tokio::test]
async fn is_defined_eval_and_sql_diverge_on_null() {
    let (_backend, handle) = TursoBackend::new_in_memory().await.expect("in-memory db");
    std::mem::forget(_backend);
    handle
        .execute_ddl("CREATE TABLE qd (id TEXT PRIMARY KEY, role TEXT)")
        .await
        .expect("create table");
    handle
        .execute("INSERT INTO qd (id, role) VALUES ('r1', NULL)", vec![])
        .await
        .expect("seed row");

    let comp = holon_api::expr_parser::parse(r#"is_def_var("role")"#).expect("must parse");

    // Seat A, on a context where the key is PRESENT and bound to Null — the
    // in-memory image of that row.
    let ctx: HashMap<String, Value> = HashMap::from([("role".to_string(), Value::Null)]);
    assert_eq!(
        comp.eval(&ctx).expect("must evaluate"),
        Value::Boolean(true),
        "eval is key-presence: a key bound to Null is defined"
    );

    // Seat B, on the row itself.
    let plan = DerivedFieldPlan::plan(vec![DerivedField::new(fid("d"), comp)]);
    let col = &plan.sql_planted[0].sql;
    assert_eq!(col, "(role IS NOT NULL)");
    reconcile_named_view(
        &handle,
        "v_isdef_null",
        &format!("SELECT id, {col} AS d FROM qd"),
    )
    .await
    .unwrap_or_else(|e| panic!("planted DDL `{col}` must succeed: {e}"));
    let rows = handle
        .query("SELECT d FROM v_isdef_null", HashMap::new())
        .await
        .expect("query planted view");
    let sql_val = rows[0].get("d").cloned().expect("d column");
    assert_eq!(
        sql_val,
        Value::Integer(0),
        "SQL cannot distinguish absent from NULL"
    );

    // The absent-key state is the one both seats DO agree on.
    assert_eq!(
        holon_api::expr_parser::parse(r#"is_def_var("role")"#)
            .unwrap()
            .eval(&HashMap::new())
            .expect("must evaluate"),
        Value::Boolean(false)
    );
}

/// v0.8 FIX GUARD (was `matview_whole_float_literal_bug_is_pinned`). The Turso
/// fork's matview logical plan used to integer-type whole-number float
/// literals, so a planted `(3.0 / 2.0)` maintained as `1` and `(xi / 2.0)` as
/// `4`, diverging from `Computation::eval` (1.5 and 4.5). The v0.8 line
/// (nightscape@holon 3ef4bece) preserves REAL affinity; this test asserts the
/// matview column now equals eval, so a regression would flip it RED. These
/// cases could be folded back into `planted_sql_matches_eval`; kept as a named
/// test to document the specific whole-float-affinity fix across the pin bump.
#[tokio::test]
async fn matview_whole_float_literal_affinity_matches_eval() {
    let handle = setup().await;
    let ctx: HashMap<String, Value> = HashMap::from([("xi".to_string(), Value::Integer(9))]);

    // literal / literal: eval is type-faithful (3.0 / 2.0 = 1.5); v0.8 matches.
    let whole = div(flit(3.0), flit(2.0));
    let expected = whole.eval(&ctx).unwrap();
    assert_eq!(expected, Value::Float(1.5));
    let sql = plant_and_read(&handle, "v_litlit", whole).await;
    assert!(
        approx_eq(&expected, &sql),
        "litlit: eval={expected:?} matview_sql={sql:?}"
    );

    // int column / whole-float literal: eval promotes to float (9 / 2.0 = 4.5);
    // v0.8 keeps REAL affinity where v0.7 integer-divided to 4.
    let mixed = div(field("xi"), flit(2.0));
    let expected = mixed.eval(&ctx).unwrap();
    assert_eq!(expected, Value::Float(4.5));
    let sql = plant_and_read(&handle, "v_intcol_wholelit", mixed).await;
    assert!(
        approx_eq(&expected, &sql),
        "intcol/wholelit: eval={expected:?} matview_sql={sql:?}"
    );
}

/// THIRD LEG persisted: the value the SIDECAR watcher lands in `block_derived`
/// must equal BOTH `Computation::eval` and the SQL-planted matview value. The
/// watcher evaluates in Rust (so sidecar == eval holds by construction); this
/// test makes the full triangle explicit end-to-end, including the persisted
/// round-trip through `block_derived.value_json`.
///
/// Fractional literal (`* 1.5`) keeps REAL affinity, so the planted leg is
/// correct. Whole-float-literal affinity is now also correct on the v0.8 line
/// and covered by `matview_whole_float_literal_affinity_matches_eval`.
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
        vec![DerivedField::new(fid("d"), comp)],
        Arc::new(ConditionBus::new()),
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

/// A NULL operand gives NULL in every seat: eval, the planted column, and the
/// sidecar. NULL is "no value", so the sidecar raises no
/// `DerivedFieldNotComputed` for it.
#[tokio::test]
async fn a_null_operand_gives_null_in_every_seat() {
    let handle = setup().await;
    BlockDerivedSchemaModule
        .ensure_schema(&handle)
        .await
        .expect("block_derived table");
    handle
        .execute_ddl("CREATE TABLE tn (id TEXT PRIMARY KEY, xi INTEGER, xn INTEGER)")
        .await
        .expect("create table");
    handle
        .execute("INSERT INTO tn (id, xi, xn) VALUES ('r1', 9, NULL)", vec![])
        .await
        .expect("seed row");
    let comp = arith(
        ArithOp::Add,
        Box::new(arith(ArithOp::Mul, field("xi"), field("xn"))),
        ilit(1),
    );
    let ctx = HashMap::from([
        ("xi".to_string(), Value::Integer(9)),
        ("xn".to_string(), Value::Null),
    ]);
    assert_eq!(comp.eval(&ctx), Ok(Value::Null), "eval");

    let plan = DerivedFieldPlan::plan(vec![DerivedField::new(fid("d"), comp.clone())]);
    let col = &plan.sql_planted[0].sql;
    reconcile_named_view(
        &handle,
        "v_null_operand",
        &format!("SELECT id, {col} AS d FROM tn"),
    )
    .await
    .unwrap_or_else(|e| panic!("planted DDL `{col}` must succeed: {e}"));
    assert_eq!(
        read_d(&handle, "v_null_operand").await,
        Ok(Value::Null),
        "planted column"
    );

    let bus = Arc::new(ConditionBus::new());
    let mgr = MatviewManager::new(handle.clone(), Arc::new(tokio::sync::Mutex::new(())));
    let _guard = spawn_derived_field_reconciler(
        &mgr,
        handle.clone(),
        "SELECT id, xi, xn FROM tn",
        vec![DerivedField::new(fid("d"), comp)],
        bus.clone(),
    )
    .await
    .expect("spawn reconciler");
    for _ in 0..100 {
        let disclosed: Vec<_> = bus
            .current()
            .into_iter()
            .filter(|c| {
                matches!(
                    c.reason,
                    holon_api::ConditionKind::DerivedFieldNotComputed { .. }
                )
            })
            .collect();
        assert!(
            disclosed.is_empty(),
            "NULL is not a failed computation: {disclosed:?}"
        );
        let rows = handle
            .query_positional(
                "SELECT value_json FROM block_derived WHERE block_id = 'r1' AND field_name = 'd'",
                vec![],
            )
            .await
            .expect("query block_derived");
        if let Some(row) = rows.first() {
            assert_eq!(
                row.get("value_json"),
                Some(&Value::String("null".into())),
                "sidecar"
            );
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the sidecar never wrote r1.d");
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

// ---------------------------------------------------------------------------
// Arithmetic with no exact finite answer: the planted column and `eval` must
// give the same value or the same refusal.
// ---------------------------------------------------------------------------

/// A cell as written. The column's declared affinity decides what is stored, so
/// a cell may be of another kind than its column: TEXT `'7'` written to an
/// INTEGER column is stored as the integer 7.
#[derive(Debug, Clone, Copy)]
enum Cell {
    Int(i64),
    Real(f64),
    Text(&'static str),
    Null,
}

impl Cell {
    fn param(self) -> turso::Value {
        match self {
            Cell::Int(i) => turso::Value::Integer(i),
            Cell::Real(f) => turso::Value::Real(f),
            Cell::Text(s) => turso::Value::Text(s.to_string()),
            Cell::Null => turso::Value::Null,
        }
    }
}

/// The operand row over [`ARITH_TABLE_COLUMNS`]; `xn` is always NULL.
#[derive(Debug, Clone)]
struct ArithRow {
    xi: Cell,
    xf: Cell,
    xs: &'static str,
    xb: Option<bool>,
}

impl ArithRow {
    fn params(&self, id: &str) -> Vec<turso::Value> {
        vec![
            turso::Value::Text(id.to_string()),
            self.xi.param(),
            self.xf.param(),
            turso::Value::Text(self.xs.to_string()),
            self.xb
                .map_or(turso::Value::Null, |b| turso::Value::Integer(b.into())),
        ]
    }
}

const ARITH_TABLE_COLUMNS: &str =
    "(id TEXT PRIMARY KEY, xi INTEGER, xf REAL, xs TEXT, xn INTEGER, xb BOOLEAN)";
const ARITH_INSERT: &str = "INSERT INTO {t} (id, xi, xf, xs, xn, xb) VALUES (?, ?, ?, ?, NULL, ?)";

/// The declared kinds of [`ARITH_TABLE_COLUMNS`], derived the way the type
/// registry derives them.
fn arith_field_types() -> FieldTypes {
    let mut types = FieldTypes::new();
    for (column, sql_type) in [
        ("xi", "INTEGER"),
        ("xf", "REAL"),
        ("xs", "TEXT"),
        ("xn", "INTEGER"),
        ("xb", "BOOLEAN"),
    ] {
        types.insert(
            column,
            FieldKind::of_sql_type(sql_type).expect("an affinity the parser knows"),
        );
    }
    types
}

static ARITH_CASE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// The stored row: what the sidecar reconciler evaluates against, after the
/// column affinity has converted each written cell.
async fn stored_context(handle: &DbHandle, table: &str) -> HashMap<String, Value> {
    let rows = handle
        .query(
            &format!("SELECT xi, xf, xs, xn, xb FROM {table}"),
            HashMap::new(),
        )
        .await
        .expect("read the stored row");
    match rows.as_slice() {
        [row] => row
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        other => panic!("{table} must hold the one row, holds {other:?}"),
    }
}

/// The stored row, and the planted column's outcome on it by both ways a
/// matview computes it: the initial population at CREATE, and IVM maintenance
/// of an INSERT into a table the view already covers.
async fn planted_outcomes(
    handle: &DbHandle,
    comp: &Computation,
    row: &ArithRow,
) -> (
    HashMap<String, Value>,
    [(&'static str, Result<Value, String>); 2],
) {
    let n = ARITH_CASE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let plan = DerivedFieldPlan::plan(vec![DerivedField::new(fid("d"), comp.clone())]);
    assert_eq!(
        plan.sql_planted.len(),
        1,
        "{comp:?} must plant; stage={:?}",
        plan.stage_evaluated
    );
    let col = &plan.sql_planted[0].sql;

    let initial_t = format!("ta_{n}");
    let ivm_t = format!("tb_{n}");
    for t in [&initial_t, &ivm_t] {
        handle
            .execute_ddl(&format!("CREATE TABLE {t} {ARITH_TABLE_COLUMNS}"))
            .await
            .expect("create table");
    }
    handle
        .execute(&ARITH_INSERT.replace("{t}", &initial_t), row.params("r1"))
        .await
        .expect("seed the initial-population table");
    let stored = stored_context(handle, &initial_t).await;

    let initial = async {
        let view = format!("va_{n}");
        reconcile_named_view(
            handle,
            &view,
            &format!("SELECT id, {col} AS d FROM {initial_t}"),
        )
        .await
        .map_err(|e| e.to_string())?;
        read_d(handle, &view).await
    }
    .await;

    let ivm = async {
        let view = format!("vb_{n}");
        reconcile_named_view(
            handle,
            &view,
            &format!("SELECT id, {col} AS d FROM {ivm_t}"),
        )
        .await
        .unwrap_or_else(|e| panic!("planted DDL `{col}` over an empty table: {e}"));
        handle
            .execute(&ARITH_INSERT.replace("{t}", &ivm_t), row.params("r1"))
            .await
            .map_err(|e| e.to_string())?;
        read_d(handle, &view).await
    }
    .await;

    (
        stored,
        [
            ("matview initial population", initial),
            ("IVM maintenance", ivm),
        ],
    )
}

async fn read_d(handle: &DbHandle, view: &str) -> Result<Value, String> {
    let rows = handle
        .query(&format!("SELECT d FROM {view}"), HashMap::new())
        .await
        .map_err(|e| e.to_string())?;
    match rows.as_slice() {
        [row] => Ok(row.get("d").cloned().expect("d column present")),
        other => panic!("{view} must hold the one row, holds {other:?}"),
    }
}

/// `eval` over the stored row is the oracle: its value must be the planted
/// value exactly, and its refusal must be the planted refusal. A refusal names
/// arithmetic that has no finite exact answer or an operand that is not a
/// number; a NULL operand gives NULL, so NULL is never the refused operand.
async fn assert_planted_matches_eval(
    handle: &DbHandle,
    comp: &Computation,
    row: &ArithRow,
) -> Result<(), String> {
    let (stored, planted) = planted_outcomes(handle, comp, row).await;
    let expected = comp.eval(&stored);
    match &expected {
        Ok(_)
        | Err(ComputeError::Arithmetic { .. })
        | Err(ComputeError::NotNumeric {
            value: Value::String(_),
            ..
        }) => {}
        Err(other) => {
            return Err(format!(
                "eval refused {comp:?} over {stored:?} for a reason arithmetic does not have: \
                 {other}"
            ));
        }
    }
    for (leg, got) in planted {
        let agree = match (&expected, &got) {
            (Ok(e), Ok(g)) => e == g,
            (Err(e), Err(g)) => g.contains(&e.to_string()),
            _ => false,
        };
        if !agree {
            return Err(format!(
                "{leg} disagrees with eval for {comp:?} over {row:?} (stored {stored:?}):\n  eval: \
                 {expected:?}\n  sql:  {got:?}"
            ));
        }
    }
    Ok(())
}

fn arith(op: ArithOp, lhs: Box<Computation>, rhs: Box<Computation>) -> Computation {
    Computation::Arith { op, lhs, rhs }
}

fn benign_row() -> ArithRow {
    ArithRow {
        xi: Cell::Int(5),
        xf: Cell::Real(1.5),
        xs: "x",
        xb: Some(true),
    }
}

#[tokio::test]
async fn arithmetic_without_a_finite_exact_answer_is_refused_by_both_seats() {
    let handle = setup().await;
    let row_with = |xi: i64, xf: f64| ArithRow {
        xi: Cell::Int(xi),
        xf: Cell::Real(xf),
        ..benign_row()
    };
    let xi_text = |text: &'static str| ArithRow {
        xi: Cell::Text(text),
        ..benign_row()
    };
    let cases: Vec<(Computation, ArithRow)> = vec![
        (div(field("xi"), ilit(0)), benign_row()),
        (
            div(
                field("xi"),
                Box::new(arith(ArithOp::Sub, field("xi"), field("xi"))),
            ),
            benign_row(),
        ),
        (div(field("xf"), flit(0.0)), benign_row()),
        (div(field("xf"), ilit(0)), benign_row()),
        (div(field("xf"), field("xf")), row_with(5, 0.0)),
        (
            arith(ArithOp::Mul, field("xf"), ilit(10)),
            row_with(5, 1e308),
        ),
        (
            arith(
                ArithOp::Sub,
                Box::new(arith(ArithOp::Mul, field("xf"), ilit(10))),
                Box::new(arith(ArithOp::Mul, field("xf"), ilit(10))),
            ),
            row_with(5, 1e308),
        ),
        (
            arith(ArithOp::Add, field("xi"), ilit(1)),
            row_with(i64::MAX, 1.5),
        ),
        (
            arith(ArithOp::Mul, field("xi"), ilit(2)),
            row_with(i64::MAX, 1.5),
        ),
        (
            arith(ArithOp::Sub, field("xi"), ilit(1)),
            row_with(i64::MIN, 1.5),
        ),
        (
            arith(ArithOp::Sub, ilit(0), field("xi")),
            row_with(i64::MIN, 1.5),
        ),
        (arith(ArithOp::Add, field("xi"), ilit(1)), xi_text("abc")),
        (arith(ArithOp::Add, field("xi"), ilit(1)), xi_text(" 5 ")),
        // Numeric text in an INTEGER column is stored as a number.
        (arith(ArithOp::Add, field("xi"), ilit(1)), xi_text("7")),
        // A NULL operand gives NULL, with or without a refusing partner.
        (arith(ArithOp::Add, field("xn"), ilit(1)), benign_row()),
        (arith(ArithOp::Mul, field("xi"), field("xn")), benign_row()),
        (div(field("xn"), ilit(0)), benign_row()),
        (
            arith(
                ArithOp::Sub,
                Box::new(Computation::Lit(Value::Null)),
                field("xf"),
            ),
            benign_row(),
        ),
        // Finite controls: the checked path keeps the ordinary answers.
        (div(field("xi"), ilit(2)), benign_row()),
        (arith(ArithOp::Add, field("xi"), field("xf")), benign_row()),
        (
            arith(ArithOp::Sub, field("xi"), ilit(1)),
            row_with(i64::MAX, 1.5),
        ),
    ];
    let mut failures = Vec::new();
    for (comp, row) in &cases {
        if let Err(e) = assert_planted_matches_eval(&handle, comp, row).await {
            failures.push(e);
        }
    }
    assert!(
        failures.is_empty(),
        "{} of {} cases disagree:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

fn arb_leaf() -> impl Strategy<Value = Computation> {
    prop_oneof![
        4 => prop::sample::select(vec!["xi", "xf", "xn", "xs", "xb"])
            .prop_map(|c| Computation::Field(c.into())),
        3 => prop::sample::select(vec![0i64, 1, 2, 7, i64::MAX, -1, -3, i64::MIN])
            .prop_map(|i| Computation::Lit(Value::Integer(i))),
        3 => prop::sample::select(vec![0.0f64, 0.5, 2.0, 1e308, -1.5, -0.0])
            .prop_map(|f| Computation::Lit(Value::Float(f))),
        1 => prop::sample::select(vec![
            Value::Null,
            Value::Boolean(true),
            Value::Boolean(false),
            Value::String("7".into()),
        ])
        .prop_map(Computation::Lit),
    ]
}

fn arb_arith() -> impl Strategy<Value = Computation> {
    let op = prop::sample::select(vec![ArithOp::Add, ArithOp::Sub, ArithOp::Mul, ArithOp::Div]);
    arb_leaf().prop_recursive(3, 8, 2, move |inner| {
        (op.clone(), inner.clone(), inner)
            .prop_map(|(op, l, r)| arith(op, Box::new(l), Box::new(r)))
    })
}

/// Cells of every kind, whatever the column's declared affinity.
fn arb_cell(ints: Vec<i64>, reals: Vec<f64>) -> impl Strategy<Value = Cell> {
    prop_oneof![
        3 => prop::sample::select(ints).prop_map(Cell::Int),
        3 => prop::sample::select(reals).prop_map(Cell::Real),
        2 => prop::sample::select(vec!["7", "2.5", "7.0", " 5 ", "abc", "inf"]).prop_map(Cell::Text),
        1 => Just(Cell::Null),
    ]
}

fn arb_row() -> impl Strategy<Value = ArithRow> {
    let ints = vec![0i64, 1, -1, 7, 3_037_000_500, i64::MAX, i64::MIN];
    let reals = vec![0.0f64, 1.5, -2.5, 2.0, 1e308, -1e308];
    (
        arb_cell(ints.clone(), reals.clone()),
        arb_cell(ints, reals),
        prop::sample::select(vec!["5", "inf", "abc"]),
        prop::option::of(any::<bool>()),
    )
        .prop_map(|(xi, xf, xs, xb)| ArithRow { xi, xf, xs, xb })
}

/// The first operand of an arithmetic node that is not a number by its
/// declaration: a column declared other than numeric, or a text or boolean
/// literal. NULL is no such operand.
fn non_numeric_operand(comp: &Computation, types: &FieldTypes) -> Option<Computation> {
    let Computation::Arith { lhs, rhs, .. } = comp else {
        return None;
    };
    [lhs, rhs].into_iter().find_map(|operand| match &**operand {
        Computation::Field(name) if types.kind(name) != Some(FieldKind::Numeric) => {
            Some((**operand).clone())
        }
        Computation::Lit(Value::Boolean(_) | Value::String(_)) => Some((**operand).clone()),
        nested => non_numeric_operand(nested, types),
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// The declaration refuses arithmetic over an operand that is not a
    /// number; every declaration it accepts plants a column that gives eval's
    /// value or eval's refusal.
    #[test]
    fn planted_arithmetic_gives_evals_value_or_evals_refusal(
        comp in arb_arith().prop_filter("an arithmetic node, not a bare leaf", |c| {
            matches!(c, Computation::Arith { .. })
        }),
        row in arb_row(),
    ) {
        let types = arith_field_types();
        match (comp.result_kind(&types), non_numeric_operand(&comp, &types)) {
            (Err(_), Some(_)) => {}
            (Ok(FieldKind::Numeric), None) => {
                let runtime = tokio::runtime::Runtime::new().expect("runtime");
                let outcome = runtime.block_on(async {
                    let handle = setup().await;
                    assert_planted_matches_eval(&handle, &comp, &row).await
                });
                prop_assert!(outcome.is_ok(), "{}", outcome.unwrap_err());
            }
            (declared, operand) => prop_assert!(
                false,
                "declaring {comp:?} gave {declared:?}; its non-numeric operand is {operand:?}"
            ),
        }
    }
}

/// A refusal during IVM maintenance refuses the base-table write as a whole:
/// the row is not stored, the view keeps its rows, and the next good write
/// lands.
#[tokio::test]
async fn a_refused_derived_value_refuses_the_base_write_and_keeps_the_view() {
    let handle = setup().await;
    handle
        .execute_ddl(&format!("CREATE TABLE tw {ARITH_TABLE_COLUMNS}"))
        .await
        .expect("create table");
    let insert = ARITH_INSERT.replace("{t}", "tw");
    handle
        .execute(&insert, benign_row().params("ok1"))
        .await
        .expect("seed");
    let plan = DerivedFieldPlan::plan(vec![DerivedField::new(
        fid("d"),
        arith(ArithOp::Add, field("xi"), ilit(1)),
    )]);
    let col = &plan.sql_planted[0].sql;
    reconcile_named_view(&handle, "vw", &format!("SELECT id, {col} AS d FROM tw"))
        .await
        .expect("planted DDL");

    let overflow = ArithRow {
        xi: Cell::Int(i64::MAX),
        ..benign_row()
    };
    let refused = handle.execute(&insert, overflow.params("bad")).await;
    let refused = refused.expect_err("an overflowing derived value must refuse the INSERT");
    assert!(
        refused
            .to_string()
            .contains("integer overflow: 9223372036854775807 + 1"),
        "the refusal must name the operation and its operands: {refused}"
    );
    let refused_update = handle
        .execute(
            "UPDATE tw SET xi = ? WHERE id = 'ok1'",
            vec![turso::Value::Integer(i64::MAX)],
        )
        .await;
    assert!(
        refused_update.is_err(),
        "an overflowing derived value must refuse the UPDATE: {refused_update:?}"
    );

    let base = handle
        .query("SELECT id, xi FROM tw ORDER BY id", HashMap::new())
        .await
        .expect("read base");
    assert_eq!(
        base.len(),
        1,
        "the refused rows must not be stored: {base:?}"
    );
    assert_eq!(base[0].get("xi"), Some(&Value::Integer(5)));
    let view = handle
        .query("SELECT id, d FROM vw ORDER BY id", HashMap::new())
        .await
        .expect("read view");
    assert_eq!(view.len(), 1, "the view must keep its rows: {view:?}");
    assert_eq!(view[0].get("d"), Some(&Value::Integer(6)));

    handle
        .execute(
            &insert,
            ArithRow {
                xi: Cell::Int(3),
                ..benign_row()
            }
            .params("ok2"),
        )
        .await
        .expect("the next good write lands");
    let view = handle
        .query("SELECT d FROM vw ORDER BY id", HashMap::new())
        .await
        .expect("read view");
    assert_eq!(view.len(), 2, "{view:?}");
    assert_eq!(view[1].get("d"), Some(&Value::Integer(4)));
}

/// A negative literal reaches planted DDL through a switch label and through a
/// programmatic `Lit`; the matview planner must accept both.
#[tokio::test]
async fn a_negative_literal_plants() {
    let handle = setup().await;
    let ctx = HashMap::from([
        ("xi".to_string(), Value::Integer(9)),
        ("xf".to_string(), Value::Float(5.0)),
    ]);
    let cases = [
        ("v_neg_int", arith(ArithOp::Mul, field("xi"), ilit(-5))),
        ("v_neg_float", arith(ArithOp::Add, field("xf"), flit(-1.5))),
        (
            "v_neg_min",
            arith(ArithOp::Add, field("xi"), ilit(i64::MIN)),
        ),
        (
            "v_neg_label",
            holon_api::expr_parser::parse("switch xi { -9 => 1, 9 => -2, _ => 3 }")
                .expect("subset parses a negative switch label"),
        ),
    ];
    for (view, comp) in cases {
        let expected = comp.eval(&ctx).expect("eval");
        let got = plant_and_read(&handle, view, comp).await;
        assert_eq!(expected, got, "{view}");
    }
}

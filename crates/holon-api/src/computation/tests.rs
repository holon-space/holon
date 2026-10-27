use super::*;

fn engine() -> rhai::Engine {
    bounded_engine()
}

fn fid(name: &str) -> FieldIdent {
    FieldIdent::parse(name).expect("test identifier")
}

fn ctx(pairs: &[(&str, Value)]) -> Context {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

#[test]
fn lit_and_field_eval() {
    let c = ctx(&[("x", Value::Integer(7))]);
    assert_eq!(
        Computation::Lit(Value::Integer(3)).eval(&c).unwrap(),
        Value::Integer(3)
    );
    assert_eq!(
        Computation::Field("x".into()).eval(&c).unwrap(),
        Value::Integer(7)
    );
}

#[test]
fn missing_field_is_loud() {
    let c = ctx(&[]);
    assert_eq!(
        Computation::Field("nope".into()).eval(&c),
        Err(ComputeError::MissingField("nope".into()))
    );
}

#[test]
fn arith_eval() {
    let c = ctx(&[("a", Value::Float(2.0)), ("b", Value::Integer(3))]);
    let expr = Computation::Arith {
        op: ArithOp::Mul,
        lhs: Box::new(Computation::Field("a".into())),
        rhs: Box::new(Computation::Arith {
            op: ArithOp::Add,
            lhs: Box::new(Computation::Field("b".into())),
            rhs: Box::new(Computation::Lit(Value::Integer(1))),
        }),
    };
    assert_eq!(expr.eval(&c).unwrap(), Value::Float(8.0)); // 2 * (3 + 1)
}

#[test]
fn embedded_predicate_eval() {
    let c = ctx(&[("done", Value::Boolean(true))]);
    let p = Computation::Predicate(Predicate::Var("done".into()));
    assert_eq!(p.eval(&c).unwrap(), Value::Boolean(true));
}

#[test]
fn script_eval_matches_rhai() {
    let expr = CompiledExpr::compile(&engine(), "priority * 10.0 + 1.0").unwrap();
    let c = ctx(&[("priority", Value::Float(3.0))]);
    assert_eq!(
        Computation::Script(expr).eval(&c).unwrap(),
        Value::Float(31.0)
    );
}

#[test]
fn script_switch_like_prototype_weight() {
    // Mirrors the default `priority_weight` prototype expression.
    let expr = CompiledExpr::compile(
        &engine(),
        "switch priority { 1.0 => 100.0, 2.0 => 40.0, 3.0 => 15.0, _ => 1.0 }",
    )
    .unwrap();
    let c = ctx(&[("priority", Value::Float(2.0))]);
    assert_eq!(
        Computation::Script(expr).eval(&c).unwrap(),
        Value::Float(40.0)
    );
}

#[test]
fn compile_sql_lowers_arith_and_field() {
    let expr = Computation::Arith {
        op: ArithOp::Mul,
        lhs: Box::new(Computation::Field("weight".into())),
        rhs: Box::new(Computation::Lit(Value::Integer(2))),
    };
    let frag = expr.compile_sql().unwrap();
    assert_eq!(frag.sql, "holon_mul(weight, ?)");
    assert_eq!(frag.params, vec![Value::Integer(2)]);
}

#[test]
fn compile_sql_predicate_and() {
    let p = Predicate::And(vec![
        Predicate::Eq {
            field: "a".into(),
            value: Value::Integer(1),
        },
        Predicate::Gt {
            field: "b".into(),
            value: Value::Integer(5),
        },
    ]);
    let frag = Computation::Predicate(p).compile_sql().unwrap();
    assert_eq!(frag.sql, "(a = ?) AND (b > ?)");
    assert_eq!(frag.params, vec![Value::Integer(1), Value::Integer(5)]);
}

#[test]
fn compile_sql_always_is_true_not_dropped() {
    // Regression: the old ToSql returned None for Always (silent no-filter).
    let frag = Computation::Predicate(Predicate::Always)
        .compile_sql()
        .unwrap();
    assert_eq!(frag.sql, "1 = 1");
}

#[test]
fn compile_sql_script_is_disclosed_not_silent() {
    // THE anti-regression for the silent hole: a script fails LOUD and NAMED.
    let expr = CompiledExpr::compile(&engine(), "if x > 1 { 2 } else { 3 }").unwrap();
    let err = Computation::Script(expr).compile_sql().unwrap_err();
    assert_eq!(
        err,
        SqlUnsupported::Script {
            source: "if x > 1 { 2 } else { 3 }".into()
        }
    );
}

/// Evaluate an ordered set of named computed fields, threading each result
/// back into the context so later fields can depend on earlier ones
/// (the reactive "computed = properties in the pipeline" shape).
/// Returns the full field map.
fn recompute_fields(inputs: &Context, fields: &[(&str, Computation)]) -> HashMap<String, Value> {
    let mut scope = inputs.clone();
    for (name, comp) in fields {
        let v = comp.eval(&scope).unwrap();
        scope.insert(name.to_string(), v);
    }
    fields
        .iter()
        .map(|(n, _)| (n.to_string(), scope[*n].clone()))
        .collect()
}

#[test]
fn end_to_end_computed_field_and_incrementality() {
    let eng = engine();
    // Derived-field program: priority_weight, then task_weight depending on it +
    // position. Mixed Script + Arith + Field shapes.
    let fields = vec![
        (
            "priority_weight",
            Computation::Script(
                CompiledExpr::compile(
                    &eng,
                    "switch priority { 1.0 => 100.0, 2.0 => 40.0, _ => 1.0 }",
                )
                .unwrap(),
            ),
        ),
        (
            "position_weight",
            Computation::Arith {
                op: ArithOp::Mul,
                lhs: Box::new(Computation::Lit(Value::Float(0.001))),
                rhs: Box::new(Computation::Field("position".into())),
            },
        ),
        (
            "task_weight",
            Computation::Arith {
                op: ArithOp::Add,
                lhs: Box::new(Computation::Field("priority_weight".into())),
                rhs: Box::new(Computation::Field("position_weight".into())),
            },
        ),
    ];

    let inputs = ctx(&[
        ("priority", Value::Float(2.0)),
        ("position", Value::Float(10.0)),
    ]);
    let before = recompute_fields(&inputs, &fields);
    assert_eq!(before["priority_weight"], Value::Float(40.0));
    assert_eq!(before["position_weight"], Value::Float(0.01));
    assert_eq!(before["task_weight"], Value::Float(40.01));

    // Incremental change: bump ONLY `priority`. `priority_weight` and its
    // dependent `task_weight` recompute; `position_weight` is untouched.
    let mut changed = inputs.clone();
    changed.insert("priority".into(), Value::Float(1.0));
    let after = recompute_fields(&changed, &fields);

    assert_eq!(after["priority_weight"], Value::Float(100.0)); // changed
    assert_eq!(after["task_weight"], Value::Float(100.01)); // dependent changed
    assert_eq!(
        after["position_weight"], before["position_weight"],
        "field independent of the changed input must not move"
    );
    assert_ne!(after["priority_weight"], before["priority_weight"]);
    assert_ne!(after["task_weight"], before["task_weight"]);
}

#[test]
fn inline_sql_inlines_literals_no_placeholders() {
    let frag = Computation::Arith {
        op: ArithOp::Mul,
        lhs: Box::new(Computation::Field("priority".into())),
        rhs: Box::new(Computation::Lit(Value::Integer(2))),
    }
    .compile_sql()
    .unwrap();
    // Parameterized form keeps `?`; inlined form is DDL-safe (no bind params).
    assert_eq!(frag.sql, "holon_mul(priority, ?)");
    assert_eq!(frag.inline_sql().unwrap(), "holon_mul(priority, 2)");
}

#[test]
fn inline_sql_escapes_string_literals() {
    let frag = SqlFragment::new("name = ?", vec![Value::String("O'Brien".into())]);
    assert_eq!(frag.inline_sql().unwrap(), "name = 'O''Brien'");
}

#[test]
fn inline_sql_rejects_non_scalar() {
    let frag = SqlFragment::new("x = ?", vec![Value::Array(vec![Value::Integer(1)])]);
    assert!(matches!(
        frag.inline_sql(),
        Err(InlineError::NonScalarLiteral { .. })
    ));
}

#[test]
fn plan_splits_sql_plantable_from_script() {
    // `weight * 2` lowers to SQL (seat A); a switch script does not (seat B).
    let sql_field = DerivedField::new(
        fid("boosted"),
        Computation::Arith {
            op: ArithOp::Mul,
            lhs: Box::new(Computation::Field("weight".into())),
            rhs: Box::new(Computation::Lit(Value::Integer(2))),
        },
    );
    let script_field = DerivedField::new(
        fid("priority_weight"),
        Computation::Script(
            CompiledExpr::compile(&engine(), "switch priority { 1.0 => 100.0, _ => 1.0 }").unwrap(),
        ),
    );
    let plan = DerivedFieldPlan::plan(vec![sql_field, script_field]);

    assert_eq!(plan.sql_planted.len(), 1);
    assert_eq!(plan.sql_planted[0].name.as_str(), "boosted");
    assert_eq!(plan.sql_planted[0].sql, "holon_mul(weight, 2)");

    assert_eq!(plan.stage_evaluated.len(), 1);
    assert_eq!(
        plan.stage_evaluated[0].field.name.as_str(),
        "priority_weight"
    );
    // Disclosed reason names the offending shape.
    assert!(
        plan.stage_evaluated[0].reason.contains("Rhai script"),
        "reason must disclose why it fell to the stage: {}",
        plan.stage_evaluated[0].reason
    );
}

#[test]
fn plan_stage_eval_is_live_and_retracts_cleanly() {
    // A Rhai-only derived field, maintained via seat B. Changing the input
    // recomputes it and REPLACES the old value (never stacks).
    let script_field = DerivedField::new(
        fid("priority_weight"),
        Computation::Script(
            CompiledExpr::compile(
                &engine(),
                "switch priority { 1.0 => 100.0, 2.0 => 40.0, _ => 1.0 }",
            )
            .unwrap(),
        ),
    );
    let plan = DerivedFieldPlan::plan(vec![script_field]);
    assert!(plan.sql_planted.is_empty());

    let mut c = ctx(&[("priority", Value::Float(2.0))]);
    plan.evaluate_stage(&mut c).unwrap();
    assert_eq!(c["priority_weight"], Value::Float(40.0));

    // Input changes: same map re-evaluated. The derived value is replaced,
    // and there is exactly one entry for the field (no stacking).
    c.insert("priority".into(), Value::Float(1.0));
    plan.evaluate_stage(&mut c).unwrap();
    assert_eq!(c["priority_weight"], Value::Float(100.0));
    assert_eq!(
        c.keys().filter(|k| *k == "priority_weight").count(),
        1,
        "recomputation must replace, not stack"
    );
}

#[test]
fn plan_stage_eval_is_fail_loud_on_missing_input() {
    // Unlike legacy resolve_computed_fields (which substitutes Null), the seat
    // surfaces a named error for a missing input.
    let field = DerivedField::new(fid("d"), Computation::Field("absent".into()));
    // Field-only computations DO lower to SQL, so force the stage path with a
    // script that references the missing input.
    let script = DerivedField::new(
        fid("d"),
        Computation::Script(CompiledExpr::compile(&engine(), "absent + 1.0").unwrap()),
    );
    let _ = field;
    let plan = DerivedFieldPlan::plan(vec![script]);
    let mut c = ctx(&[]);
    assert!(plan.evaluate_stage(&mut c).is_err());
}

fn f(name: &str) -> Box<Computation> {
    Box::new(Computation::Field(name.into()))
}

fn lit(x: f64) -> Box<Computation> {
    Box::new(Computation::Lit(Value::Float(x)))
}

#[test]
fn compare_eval_and_sql() {
    let c = ctx(&[("a", Value::Float(3.0)), ("b", Value::Float(5.0))]);
    let gt = Computation::Compare {
        op: CmpOp::Gt,
        lhs: f("a"),
        rhs: f("b"),
    };
    assert_eq!(gt.eval(&c).unwrap(), Value::Boolean(false));
    assert_eq!(gt.compile_sql().unwrap().inline_sql().unwrap(), "(a > b)");
}

#[test]
fn case_switch_eval_matches_first_equal_branch() {
    // switch priority { 1.0 => 100.0, 2.0 => 40.0, _ => 1.0 }
    let case = Computation::Case {
        scrutinee: f("priority"),
        branches: vec![
            (Computation::Lit(Value::Float(1.0)), *lit(100.0)),
            (Computation::Lit(Value::Float(2.0)), *lit(40.0)),
        ],
        else_: lit(1.0),
    };
    assert_eq!(
        case.eval(&ctx(&[("priority", Value::Float(2.0))])).unwrap(),
        Value::Float(40.0)
    );
    assert_eq!(
        case.eval(&ctx(&[("priority", Value::Float(9.0))])).unwrap(),
        Value::Float(1.0) // falls to else
    );
}

#[test]
fn case_lowers_to_nested_iif_not_case_when() {
    // The spike-mandated lowering: CASE is rejected by the fork IVM, iif is not.
    let case = Computation::Case {
        scrutinee: f("priority"),
        branches: vec![
            (Computation::Lit(Value::Float(1.0)), *lit(100.0)),
            (Computation::Lit(Value::Float(2.0)), *lit(40.0)),
        ],
        else_: lit(1.0),
    };
    let sql = case.compile_sql().unwrap().inline_sql().unwrap();
    assert_eq!(
        sql,
        "iif(priority = 1.0, 100.0, iif(priority = 2.0, 40.0, 1.0))"
    );
    assert!(!sql.contains("CASE"), "must NOT emit SQL CASE");
}

#[test]
fn case_if_shape_uses_boolean_scrutinee() {
    // if a > b { 10.0 } else { 20.0 }  == Case over scrutinee `true`.
    let case = Computation::Case {
        scrutinee: Box::new(Computation::Lit(Value::Boolean(true))),
        branches: vec![(
            Computation::Compare {
                op: CmpOp::Gt,
                lhs: f("a"),
                rhs: f("b"),
            },
            *lit(10.0),
        )],
        else_: lit(20.0),
    };
    assert_eq!(
        case.eval(&ctx(&[("a", Value::Float(7.0)), ("b", Value::Float(1.0))]))
            .unwrap(),
        Value::Float(10.0)
    );
    assert_eq!(
        case.compile_sql().unwrap().inline_sql().unwrap(),
        "iif(1 = (a > b), 10.0, 20.0)"
    );
}

#[test]
fn case_referenced_fields_walks_all_arms() {
    let case = Computation::Case {
        scrutinee: f("s"),
        branches: vec![(*f("m"), *f("r"))],
        else_: f("e"),
    };
    let fields = case.referenced_fields();
    assert!(["s", "m", "r", "e"].iter().all(|n| fields.contains(*n)));
}

fn ilit(i: i64) -> Box<Computation> {
    Box::new(Computation::Lit(Value::Integer(i)))
}

fn arith(op: ArithOp, l: Box<Computation>, r: Box<Computation>) -> Computation {
    Computation::Arith { op, lhs: l, rhs: r }
}

#[test]
fn int_arithmetic_is_type_faithful_integer_division() {
    // 5 / 2 = 2 (integer division), matching Rhai; NOT 2.5.
    let e = arith(ArithOp::Div, ilit(5), ilit(2));
    assert_eq!(e.eval(&ctx(&[])).unwrap(), Value::Integer(2));
    // 9 / 4 + 1 = 2 + 1 = 3.
    let e = arith(
        ArithOp::Add,
        Box::new(arith(ArithOp::Div, ilit(9), ilit(4))),
        ilit(1),
    );
    assert_eq!(e.eval(&ctx(&[])).unwrap(), Value::Integer(3));
}

#[test]
fn mixed_int_float_arithmetic_promotes_to_float() {
    // 5 / 2.0 = 2.5 (mixed promotes), matching Rhai.
    let e = arith(ArithOp::Div, ilit(5), lit(2.0));
    assert_eq!(e.eval(&ctx(&[])).unwrap(), Value::Float(2.5));
}

#[test]
fn integer_division_by_zero_is_fail_loud() {
    let e = arith(ArithOp::Div, ilit(5), ilit(0));
    assert!(matches!(
        e.eval(&ctx(&[])),
        Err(ComputeError::Arithmetic { .. })
    ));
}

#[test]
fn a_non_finite_float_result_is_fail_loud() {
    for (e, shown) in [
        (arith(ArithOp::Mul, lit(f64::MAX), lit(2.0)), "inf"),
        (arith(ArithOp::Div, lit(1.0), lit(0.0)), "inf"),
        (arith(ArithOp::Div, lit(0.0), lit(0.0)), "NaN"),
    ] {
        match e.eval(&ctx(&[])) {
            Err(ComputeError::Arithmetic { detail }) => {
                assert!(detail.contains(shown), "{detail}")
            }
            other => panic!("{e:?} must be refused, got {other:?}"),
        }
    }
}

#[test]
fn a_non_finite_script_result_is_fail_loud() {
    let expr = CompiledExpr::compile(&engine(), "1.0 / 0.0").unwrap();
    match Computation::Script(expr).eval(&ctx(&[])) {
        Err(ComputeError::Arithmetic { detail }) => assert!(detail.contains("inf"), "{detail}"),
        other => panic!("1.0 / 0.0 must be refused, got {other:?}"),
    }
}

#[test]
fn integer_overflow_is_fail_loud() {
    let e = arith(ArithOp::Add, ilit(i64::MAX), ilit(1));
    assert!(matches!(
        e.eval(&ctx(&[])),
        Err(ComputeError::Arithmetic { .. })
    ));
}

#[test]
fn whole_float_literal_plants_with_decimal_point() {
    // 3.0 / 2.0 must plant as `(3.0 / 2.0)` (=1.5), NOT `(3 / 2)` (=1 in
    // SQLite integer division). This is the eval-vs-SQL refutation fix.
    let e = arith(ArithOp::Div, lit(3.0), lit(2.0));
    assert_eq!(
        e.compile_sql().unwrap().inline_sql().unwrap(),
        "holon_div(3.0, 2.0)"
    );
}

#[test]
fn case_switch_is_type_strict_like_rhai() {
    // switch (int 2) { 2.0 => 100, _ => 1 } does NOT match the float case.
    let case = Computation::Case {
        scrutinee: ilit(2),
        branches: vec![(Computation::Lit(Value::Float(2.0)), *ilit(100))],
        else_: ilit(1),
    };
    assert_eq!(case.eval(&ctx(&[])).unwrap(), Value::Integer(1));
}

#[test]
fn non_finite_float_literal_is_a_loud_plant_error() {
    let frag = SqlFragment::new("x = ?", vec![Value::Float(f64::INFINITY)]);
    assert!(matches!(
        frag.inline_sql(),
        Err(InlineError::NonFiniteFloat { .. })
    ));
    let nan = SqlFragment::new("x = ?", vec![Value::Float(f64::NAN)]);
    assert!(matches!(
        nan.inline_sql(),
        Err(InlineError::NonFiniteFloat { .. })
    ));
    let second = SqlFragment::new(
        "y = ? AND x = ?",
        vec![Value::Integer(1), Value::Float(f64::INFINITY)],
    );
    let err = second.inline_sql().unwrap_err().to_string();
    assert!(
        err.contains("placeholder 2 of `y = ? AND x = ?`"),
        "the refusal names the placeholder and its SQL: {err}"
    );
}

#[test]
fn compile_sql_and_with_script_child_propagates_error() {
    // An And that embeds a script term must NOT silently drop it. Here the script
    // is inside a Computation::And-equivalent via Arith to prove propagation.
    let script = CompiledExpr::compile(&engine(), "x + 1").unwrap();
    let expr = Computation::Arith {
        op: ArithOp::Add,
        lhs: Box::new(Computation::Field("y".into())),
        rhs: Box::new(Computation::Script(script)),
    };
    assert!(matches!(
        expr.compile_sql(),
        Err(SqlUnsupported::Script { .. })
    ));
}

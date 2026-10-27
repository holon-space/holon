use super::*;

fn parse(source: &str) -> Result<RenderExpr> {
    parse_render_dsl(source)
}

/// A non-finite literal has no JSON form — `serde_json` writes it as
/// `null` and reports success, so a node prop would carry a different
/// expression than the author wrote.
#[test]
fn a_non_finite_float_literal_is_refused() {
    for source in ["text(1e400)", "text(-1e400)", "text(1e309 * 1.0)"] {
        let err = parse(source)
            .map(|expr| format!("{expr:?}"))
            .expect_err(&format!("{source} must not parse into a render expression"));
        assert!(
            format!("{err:#}").contains("finite"),
            "{source}: the refusal must name the problem; got {err:#}"
        );
    }
}

fn col(name: &str) -> Box<RenderExpr> {
    Box::new(RenderExpr::ColumnRef {
        name: name.to_string(),
    })
}

fn lit(value: Value) -> Box<RenderExpr> {
    Box::new(literal(value))
}

#[test]
fn an_operator_over_a_column_is_a_per_row_node() {
    assert_eq!(
        parse(r#"col("q") * 2"#).unwrap(),
        RenderExpr::BinaryOp {
            op: BinaryOperator::Mul,
            left: col("q"),
            right: lit(Value::Integer(2)),
        }
    );
    assert_eq!(
        parse(r#"col("q") + " g""#).unwrap(),
        RenderExpr::BinaryOp {
            op: BinaryOperator::Concat,
            left: col("q"),
            right: lit(Value::String(" g".into())),
        }
    );
    assert_eq!(
        parse(r#"!col("done")"#).unwrap(),
        RenderExpr::Not {
            operand: col("done")
        }
    );
    assert_eq!(
        parse(r#"if col("done") { "x" } else { "" }"#).unwrap(),
        RenderExpr::If {
            condition: col("done"),
            then: lit(Value::String("x".into())),
            otherwise: lit(Value::String(String::new())),
        }
    );
}

#[test]
fn a_template_concatenates_its_parts() {
    assert_eq!(
        parse(r#"`${col("q")} g`"#).unwrap(),
        RenderExpr::BinaryOp {
            op: BinaryOperator::Concat,
            left: col("q"),
            right: lit(Value::String(" g".into())),
        }
    );
}

#[test]
fn printing_a_parsed_source_is_a_fixed_point() {
    for source in [
        r#"text(`${col("q")} ${col("unit")}`)"#,
        r#"text(`${col("q")}`)"#,
        r#"text(`${col("a")}${col("b")}${col("c")}`)"#,
        "text(2 * 3.5)",
        "text(1.0e300)",
        r#"text("a \"quoted\" \\ line\n")"#,
        r#"navigation_focus(#{region: "main", block_id: col("id")})"#,
        r#"text(col("a\"b"))"#,
        r#"text(col("a\\b"))"#,
        r#"text(live_block("a\"b"))"#,
        r#"text("x", #{"a b": "c"})"#,
        r#"text([#{"a-b": 1}])"#,
        r#"text([#{"if": 1}])"#,
        r#"text([#{alpha: 1, beta: 2, gamma: 3, delta: 4, epsilon: 5, zeta: #{y: 1, x: 2}}])"#,
    ] {
        let first = parse(source).unwrap();
        let second = parse(&first.to_rhai())
            .unwrap_or_else(|e| panic!("{source}: printed {:?}: {e:#}", first.to_rhai()));
        assert_eq!(second, first, "{source}");
        assert_eq!(second.to_rhai(), first.to_rhai(), "{source}");
    }
}

#[test]
fn a_json_call_with_an_unaliased_dotted_name_is_refused_by_name() {
    for name in ["block.set_field", "whatever.thing"] {
        let json = format!(r#"{{"FunctionCall":{{"name":"{name}","args":[]}}}}"#);
        let err = format!("{:#}", parse(&json).expect_err(name));
        assert!(
            err.contains(name) && err.contains("DOTTED_ALIASES"),
            "{name}: {err}"
        );
    }
    let aliased = parse(r#"{"FunctionCall":{"name":"navigation.focus","args":[]}}"#).unwrap();
    assert_eq!(aliased.to_rhai(), "navigation_focus()");
}

#[test]
fn an_operator_over_literals_is_decided_at_load() {
    assert_eq!(parse("text(2 * 3 + 1)").unwrap().to_rhai(), "text(7)");
    assert_eq!(parse("text(2 * 3.5)").unwrap().to_rhai(), "text(7.0)");
    assert_eq!(
        parse(r#"text(if 1 < 2 { "a" } else { "b" })"#)
            .unwrap()
            .to_rhai(),
        r#"text("a")"#
    );
    assert!(
        parse("text(1 / 0)").is_err(),
        "a constant error is a load error"
    );
}

#[test]
fn a_form_with_no_per_row_node_is_refused_by_name() {
    for (source, form) in [
        ("text(x)", "a variable"),
        (r#"text(col("q").len())"#, "a method call"),
        (r#"text(col("q")[0])"#, "an index"),
        (r#"text(col("q") ?? "x")"#, "`??`"),
        (r#"text(col("q") % 2)"#, "operator `%`"),
        (r#"text(col(col("q")))"#, "`col` without one string literal"),
    ] {
        let err = format!("{:#}", parse(source).expect_err(source));
        assert!(err.contains(form), "{source}: expected {form:?} in {err}");
    }
}

#[test]
fn a_call_takes_any_number_of_arguments() {
    let source = r#"row(icon("a"), spacer(6), text("b"), spacer(8), badge("c"), spacer(4), text("d"), text("e"))"#;
    let RenderExpr::FunctionCall { args, .. } = parse(source).unwrap() else {
        panic!("{source} must parse into a call");
    };
    assert_eq!(args.len(), 8);
}

#[test]
fn test_simple_function() {
    let expr = parse("table()").unwrap();
    assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
}

#[test]
fn test_live_block() {
    let expr = parse("live_block()").unwrap();
    assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "live_block"));
}

#[test]
fn test_columns_with_named_args() {
    let expr = parse("columns(#{gap: 4, item_template: live_block()})").unwrap();
    if let RenderExpr::FunctionCall { name, args, .. } = &expr {
        assert_eq!(name, "columns");
        assert!(args.iter().any(|a| a.name.as_deref() == Some("gap")));
        assert!(
            args.iter()
                .any(|a| a.name.as_deref() == Some("item_template"))
        );
    } else {
        panic!("Expected FunctionCall");
    }
}

#[test]
fn test_col_reference() {
    let expr = parse("text(col(\"name\"))").unwrap();
    if let RenderExpr::FunctionCall { args, .. } = &expr {
        assert!(matches!(&args[0].value, RenderExpr::ColumnRef { name } if name == "name"));
    } else {
        panic!("Expected FunctionCall");
    }
}

#[test]
fn test_nested_functions() {
    let expr = parse_render_dsl(r#"row(icon("folder"), spacer(6), text(col("name")))"#).unwrap();
    if let RenderExpr::FunctionCall { name, args, .. } = &expr {
        assert_eq!(name, "row");
        assert_eq!(args.len(), 3);
    } else {
        panic!("Expected FunctionCall");
    }
}

#[test]
fn test_tree_with_col_refs() {
    let expr = parse(r#"tree(#{parent_id: col("parent_id"), sortkey: col("id"), item_template: render_entity()})"#).unwrap();
    if let RenderExpr::FunctionCall { name, args, .. } = &expr {
        assert_eq!(name, "tree");
        let parent_arg = args
            .iter()
            .find(|a| a.name.as_deref() == Some("parent_id"))
            .unwrap();
        assert!(matches!(&parent_arg.value, RenderExpr::ColumnRef { name } if name == "parent_id"));
    } else {
        panic!("Expected FunctionCall");
    }
}

#[test]
fn test_json_backwards_compat() {
    // Old JSON with operations field should still parse (serde ignores unknown
    // fields)
    let json = r#"{"FunctionCall":{"name":"table","args":[],"operations":[]}}"#;
    let expr = parse(json).unwrap();
    assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));

    let json = r#"{"FunctionCall":{"name":"table","args":[]}}"#;
    let expr = parse(json).unwrap();
    assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
}

#[test]
fn test_empty_defaults_to_table() {
    let expr = parse("").unwrap();
    assert!(matches!(expr, RenderExpr::FunctionCall { ref name, .. } if name == "table"));
}

#[test]
fn test_widget_with_integer_arg() {
    let expr = parse("chain_ops(0)").unwrap();
    if let RenderExpr::FunctionCall { name, args, .. } = &expr {
        assert_eq!(name, "chain_ops");
        assert_eq!(args.len(), 1);
    } else {
        panic!("Expected FunctionCall");
    }
}

/// A new widget name parses without an enum, parser, or registry change.
#[test]
fn new_widget_parses_without_explicit_registration() {
    let expr =
        parse_render_dsl(r#"kanban(#{group_by: "task_state", item_template: render_entity()})"#)
            .unwrap();
    match expr {
        RenderExpr::FunctionCall { name, args } => {
            assert_eq!(name, "kanban");
            assert!(args.iter().any(|a| a.name.as_deref() == Some("group_by")));
            assert!(
                args.iter()
                    .any(|a| a.name.as_deref() == Some("item_template"))
            );
        }
        other => panic!("Expected FunctionCall(kanban), got {other:?}"),
    }
}

#[test]
fn nested_new_widgets_parse_without_registration() {
    let expr = parse_render_dsl(
        r#"calendar(#{view: "month", item_template: holiday_badge(text(col("title")))})"#,
    )
    .unwrap();
    if let RenderExpr::FunctionCall { name, args } = &expr {
        assert_eq!(name, "calendar");
        // The nested holiday_badge widget should have parsed too.
        let item_template = args
            .iter()
            .find(|a| a.name.as_deref() == Some("item_template"))
            .expect("calendar has item_template");
        assert!(matches!(
            &item_template.value,
            RenderExpr::FunctionCall { name, .. } if name == "holiday_badge"
        ));
    } else {
        panic!("Expected FunctionCall");
    }
}

// ── Predicate constructors ─────────────────────────────────
//
// Each constructor's output `RenderExpr` is evaluated to a `Value`, then
// round-tripped through serde_json into a `Predicate` — the same path
// `parse_rules_arg` (render_interpreter.rs) takes for `rules: [...]`.

fn parse_predicate(source: &str) -> crate::predicate::Predicate {
    super::parse_predicate(source).expect("parse predicate DSL")
}

#[test]
fn predicate_eq() {
    match parse_predicate(r#"eq("level", 0)"#) {
        crate::predicate::Predicate::Eq { field, value } => {
            assert_eq!(field, "level");
            assert_eq!(value, crate::Value::Integer(0));
        }
        other => panic!("expected Eq, got {other:?}"),
    }
}

#[test]
fn predicate_ne_gt_lt_gte_lte() {
    // All five share a shape; spot-check each variant rather than
    // five copies of the same matching boilerplate.
    use crate::predicate::Predicate::*;
    assert!(matches!(
        parse_predicate(r#"ne("status", "done")"#),
        Ne { .. }
    ));
    assert!(matches!(parse_predicate(r#"gt("count", 3)"#), Gt { .. }));
    assert!(matches!(parse_predicate(r#"lt("count", 3)"#), Lt { .. }));
    assert!(matches!(parse_predicate(r#"gte("count", 3)"#), Gte { .. }));
    assert!(matches!(parse_predicate(r#"lte("count", 3)"#), Lte { .. }));
}

#[test]
fn predicate_is_not_null() {
    use crate::predicate::Predicate::*;
    match parse_predicate(r#"is_not_null("task_state")"#) {
        IsNotNull(field) => assert_eq!(field, "task_state"),
        other => panic!("expected IsNotNull, got {other:?}"),
    }
}

#[test]
fn predicate_var_via_quoted_verbose_form() {
    // `var` is a Rhai reserved keyword (the tokenizer rejects it
    // even inside map literals as an unquoted identifier), so we
    // don't register a constructor for it. Verify the quoted-key
    // verbose form `#{"var": "<field>"}` still round-trips into
    // `Predicate::Var`.
    use crate::predicate::Predicate::*;
    match parse_predicate(r#"#{"var": "is_focused"}"#) {
        Var(field) => assert_eq!(field, "is_focused"),
        other => panic!("expected Var, got {other:?}"),
    }
}

#[test]
fn predicate_not() {
    use crate::predicate::Predicate::*;
    match parse_predicate(r#"not(eq("level", 0))"#) {
        Not(inner) => match *inner {
            Eq { field, .. } => assert_eq!(field, "level"),
            other => panic!("expected Not(Eq), got Not({other:?})"),
        },
        other => panic!("expected Not, got {other:?}"),
    }
}

#[test]
fn predicate_and_or_variadic() {
    use crate::predicate::Predicate::*;
    match parse_predicate(r#"and(eq("level", 0), gt("depth", 0))"#) {
        And(children) => {
            assert_eq!(children.len(), 2);
            assert!(matches!(&children[0], Eq { .. }));
            assert!(matches!(&children[1], Gt { .. }));
        }
        other => panic!("expected And, got {other:?}"),
    }
    // Three-arity to confirm the additional overloads are wired.
    match parse_predicate(r#"or(eq("a", 1), eq("b", 2), eq("c", 3))"#) {
        Or(children) => assert_eq!(children.len(), 3),
        other => panic!("expected Or, got {other:?}"),
    }
}

#[test]
fn predicate_always() {
    assert!(matches!(
        parse_predicate("always()"),
        crate::predicate::Predicate::Always
    ));
}

#[test]
fn rules_arg_round_trip_through_full_tree_call() {
    // Mirror the path `parse_rules_arg` takes on a real `tree(rules: [...])`
    // call: parse the DSL, find the `rules:` arg, evaluate it as a Value,
    // and deserialize each item as a `RuleSpec`. Confirms the constructors
    // compose with the rest of the DSL.
    let expr = parse(
        r#"tree(#{
                parent_id: col("parent_id"),
                rules: [
                    #{
                        when: and(eq("level", 0), gt("depth", 0)),
                        override: #{role: "page_title", show_bullet: false}
                    },
                    #{when: always(), override: #{}}
                ]
            })"#,
    )
    .unwrap();

    let RenderExpr::FunctionCall { args, .. } = expr else {
        panic!("expected tree(...) FunctionCall");
    };
    let rules_arg = args
        .iter()
        .find(|a| a.name.as_deref() == Some("rules"))
        .expect("rules: arg present");
    let rules_value = crate::render_eval::eval_plain_value(
        &rules_arg.value,
        &HashMap::<String, crate::Value>::new(),
    )
    .unwrap();
    let crate::Value::Array(items) = rules_value else {
        panic!("rules: must evaluate to Array");
    };

    let specs: Vec<crate::render_types::RuleSpec> = items
        .iter()
        .map(|item| {
            let json = serde_json::to_value(item).expect("Value → JSON");
            serde_json::from_value(json).expect("JSON → RuleSpec")
        })
        .collect();

    assert_eq!(specs.len(), 2);

    use crate::predicate::Predicate::*;
    match &specs[0].when {
        And(children) => {
            assert!(matches!(&children[0], Eq { field, .. } if field == "level"));
            assert!(matches!(&children[1], Gt { field, .. } if field == "depth"));
        }
        other => panic!("expected And in first rule, got {other:?}"),
    }
    assert_eq!(
        specs[0].overrides.get("role").and_then(|v| v.as_string()),
        Some("page_title")
    );
    assert_eq!(
        specs[0].overrides.get("show_bullet"),
        Some(&crate::Value::Boolean(false))
    );

    assert!(matches!(specs[1].when, Always));
}

fn eval_binary_op(op: &BinaryOperator, l: &Value, r: &Value) -> Value {
    super::eval_binary_op(op, l, r).unwrap()
}

use super::*;

fn empty_args() -> ResolvedArgs {
    ResolvedArgs {
        positional: vec![],
        positional_exprs: vec![],
        named: HashMap::new(),
        rows: HashMap::new(),
        templates: HashMap::new(),
        widget: None,
    }
}

#[test]
fn sort_value_orders_ints_floats_strings_and_missing() {
    let sv = |v: &Value| sort_value(Some(v));

    assert!(sv(&Value::Integer(2)) < sv(&Value::Integer(10)));
    assert!(sv(&Value::Integer(0)) < sv(&Value::Integer(7)));

    // IEEE bit-flip trick must order negatives < zero < positives.
    assert!(sv(&Value::Float(-2.5)) < sv(&Value::Float(-1.5)));
    assert!(sv(&Value::Float(-1.5)) < sv(&Value::Float(0.0)));
    assert!(sv(&Value::Float(0.0)) < sv(&Value::Float(2.5)));
    assert!(sv(&Value::Float(2.5)) < sv(&Value::Float(10.25)));

    // FractionalIndex hex strings pass through untouched.
    assert_eq!(sv(&Value::String("7F80".to_string())), "7F80");

    // Missing sorts after any string/int representation.
    let missing = sort_value(None);
    assert_eq!(missing, "\u{10FFFF}");
    assert!(sv(&Value::String("zz".to_string())) < missing);
    assert!(sv(&Value::Integer(i64::MAX)) < missing);
}

#[test]
fn cmp_values_total_order() {
    use std::cmp::Ordering::*;
    let int = |i: i64| Value::Integer(i);
    let f = |x: f64| Value::Float(x);
    let s = |x: &str| Value::String(x.to_string());

    assert_eq!(cmp_values(Some(&int(1)), Some(&int(2))), Less);
    assert_eq!(cmp_values(Some(&int(2)), Some(&int(1))), Greater);
    assert_eq!(cmp_values(Some(&f(1.5)), Some(&f(2.5))), Less);
    assert_eq!(cmp_values(Some(&s("a")), Some(&s("b"))), Less);
    assert_eq!(cmp_values(None, None), Equal);
    assert_eq!(cmp_values(None, Some(&int(1))), Greater);
    assert_eq!(cmp_values(Some(&int(1)), None), Less);
}

#[test]
fn non_finite_float_arithmetic_result_is_refused() {
    let f = |x: f64| Value::Float(x);
    for (op, a, b, symbol) in [
        (BinaryOperator::Mul, 1e308, 10.0, "*"),
        (BinaryOperator::Add, 1.7e308, 1.7e308, "+"),
        (BinaryOperator::Sub, -1.7e308, 1.7e308, "-"),
        (BinaryOperator::Div, 1e308, 1e-308, "/"),
    ] {
        let err = super::eval_binary_op(&op, &f(a), &f(b))
            .expect_err("overflow to infinity must be refused");
        let message = err.to_string();
        assert!(
            message.contains("non-finite float") && message.contains(symbol),
            "{a:?} {symbol} {b:?}: error must name value and operator, got {message}"
        );
    }
}

#[test]
fn integer_overflow_is_refused_naming_operator_and_operands() {
    let i = |x: i64| Value::Integer(x);
    for (op, a, b, expected) in [
        (
            BinaryOperator::Add,
            i64::MAX,
            1,
            format!("{} + 1", i64::MAX),
        ),
        (
            BinaryOperator::Sub,
            i64::MIN,
            1,
            format!("{} - 1", i64::MIN),
        ),
        (
            BinaryOperator::Mul,
            i64::MAX,
            2,
            format!("{} * 2", i64::MAX),
        ),
        (
            BinaryOperator::Div,
            i64::MIN,
            -1,
            format!("{} / -1", i64::MIN),
        ),
    ] {
        let err =
            super::eval_binary_op(&op, &i(a), &i(b)).expect_err("integer overflow must be refused");
        let message = err.to_string();
        assert!(
            message.contains("integer overflow") && message.contains(&expected),
            "{expected}: error must name operator and operands, got {message}"
        );
    }
}

#[test]
fn division_by_zero_is_refused_naming_the_operands() {
    let i = |x: i64| Value::Integer(x);
    let f = |x: f64| Value::Float(x);
    for (l, r, expected) in [
        (i(7), i(0), "integer division by zero: 7 / 0"),
        (f(1.0), f(0.0), "non-finite float inf from 1.0 / 0.0"),
    ] {
        let err = super::eval_binary_op(&BinaryOperator::Div, &l, &r)
            .expect_err("division by zero must be refused");
        assert!(err.to_string().contains(expected), "{expected}: got {err}");
    }
}

#[test]
fn nested_overflow_reaches_the_caller_of_eval_plain_value() {
    let expr = RenderExpr::Array {
        items: vec![RenderExpr::BinaryOp {
            op: BinaryOperator::Mul,
            left: Box::new(RenderExpr::Literal {
                value: Value::Float(1e308),
            }),
            right: Box::new(RenderExpr::Literal {
                value: Value::Float(10.0),
            }),
        }],
    };
    let err = eval_plain_value(&expr, &HashMap::<String, Value>::new())
        .expect_err("a nested non-finite result must not become Null");
    assert!(err.to_string().contains("non-finite float inf"), "{err}");
}

#[test]
fn binary_op_arithmetic_semantics() {
    let i = |x: i64| Value::Integer(x);
    let f = |x: f64| Value::Float(x);
    let s = |x: &str| Value::String(x.to_string());

    assert_eq!(eval_binary_op(&BinaryOperator::Add, &i(2), &i(3)), i(5));
    assert_eq!(
        eval_binary_op(&BinaryOperator::Add, &f(1.5), &f(2.25)),
        f(3.75)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Concat, &s("a"), &s("b")),
        s("ab")
    );

    assert_eq!(eval_binary_op(&BinaryOperator::Sub, &i(5), &i(2)), i(3));
    assert_eq!(
        eval_binary_op(&BinaryOperator::Sub, &f(5.5), &f(2.0)),
        f(3.5)
    );

    assert_eq!(eval_binary_op(&BinaryOperator::Mul, &i(3), &i(4)), i(12));
    assert_eq!(
        eval_binary_op(&BinaryOperator::Mul, &f(1.5), &f(2.0)),
        f(3.0)
    );

    assert_eq!(eval_binary_op(&BinaryOperator::Div, &i(7), &i(2)), i(3));
    assert_eq!(
        eval_binary_op(&BinaryOperator::Div, &f(3.0), &f(2.0)),
        f(1.5)
    );
    assert_eq!(eval_binary_op(&BinaryOperator::Add, &i(1), &f(1.0)), f(2.0));
}

#[test]
fn binary_op_ordering_semantics() {
    let i = |x: i64| Value::Integer(x);
    let f = |x: f64| Value::Float(x);
    let b = |x: bool| Value::Boolean(x);

    assert_eq!(eval_binary_op(&BinaryOperator::Gt, &i(3), &i(2)), b(true));
    assert_eq!(eval_binary_op(&BinaryOperator::Gt, &i(2), &i(2)), b(false));
    assert_eq!(
        eval_binary_op(&BinaryOperator::Gt, &f(2.5), &f(2.0)),
        b(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Gt, &f(2.0), &f(2.0)),
        b(false)
    );

    assert_eq!(eval_binary_op(&BinaryOperator::Lt, &i(1), &i(2)), b(true));
    assert_eq!(eval_binary_op(&BinaryOperator::Lt, &i(2), &i(2)), b(false));
    assert_eq!(
        eval_binary_op(&BinaryOperator::Lt, &f(1.0), &f(2.0)),
        b(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Lt, &f(2.0), &f(2.0)),
        b(false)
    );

    assert_eq!(eval_binary_op(&BinaryOperator::Gte, &i(2), &i(2)), b(true));
    assert_eq!(eval_binary_op(&BinaryOperator::Gte, &i(1), &i(2)), b(false));
    assert_eq!(
        eval_binary_op(&BinaryOperator::Gte, &f(2.0), &f(2.0)),
        b(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Gte, &f(1.0), &f(2.0)),
        b(false)
    );

    assert_eq!(eval_binary_op(&BinaryOperator::Lte, &i(2), &i(2)), b(true));
    assert_eq!(eval_binary_op(&BinaryOperator::Lte, &i(3), &i(2)), b(false));
    assert_eq!(
        eval_binary_op(&BinaryOperator::Lte, &f(2.0), &f(2.0)),
        b(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Lte, &f(3.0), &f(2.0)),
        b(false)
    );
}

#[test]
fn state_and_color_display_tables() {
    assert_eq!(state_icon(""), "");
    assert_eq!(state_icon("CANCELLED"), "✗");
    assert_eq!(state_icon("DOING"), "◑");
    assert_eq!(state_icon("DONE"), "✓");
    assert_eq!(state_icon("TODO"), "○");

    assert_eq!(state_display(""), ("", ThemeToken::Muted));
    assert_eq!(state_display("TODO"), ("TODO", ThemeToken::Muted));
    assert_eq!(state_display("DOING"), ("DOING", ThemeToken::Warning));
    assert_eq!(state_display("DONE"), ("[x]", ThemeToken::Success));
    assert_eq!(state_display("CANCELLED"), ("CANCELLED", ThemeToken::Error));
    assert_eq!(state_display("WAITING"), ("WAITING", ThemeToken::Primary));
}

#[test]
fn text_style_font_size_maps_headings_and_fails_loud_on_unknown() {
    // The heading type scale is monotonically decreasing h1 > h2 > h3, and
    // every heading is strictly larger than the 14px body default so a
    // `#{style: "h1"}` title can never render at body size.
    let h1 = text_style_font_size("h1").expect("h1 is a known style");
    let h2 = text_style_font_size("h2").expect("h2 is a known style");
    let h3 = text_style_font_size("h3").expect("h3 is a known style");
    assert!(h1 > h2 && h2 > h3, "heading scale must decrease h1>h2>h3");
    assert!(h3 > 14.0, "even h3 must exceed the 14px body default");
    assert_eq!(h1, 28.0, "h1 pins the page-title heading scale");
    // Unknown keyword returns None (caller fails loud) rather than silently
    // resolving to body size — the exact swallow that hid the title bug.
    assert_eq!(text_style_font_size("h7"), None);
    assert_eq!(text_style_font_size(""), None);
}

#[test]
fn text_style_treatment_bolds_only_recognized_headings() {
    // A heading carries both halves of the treatment.
    let h1 = text_style_treatment("h1").expect("h1 is a known style");
    assert_eq!(h1.size, text_style_font_size("h1").unwrap());
    assert!(h1.bold, "a heading is bold");
    // `body` is in the scale but is not a heading.
    assert!(!text_style_treatment("body").expect("body is known").bold);
    // An unrecognized keyword yields NO treatment — not a bold body-size
    // one. `h7` looks like a heading and is not in the scale; resolving
    // size and weight separately let one frontend bold it while another
    // did not.
    assert_eq!(text_style_treatment("h7"), None);
    assert_eq!(text_style_treatment(""), None);
}

#[test]
fn text_display_shows_disclosed_placeholder_only_for_empty_content() {
    // Non-empty content always wins — the placeholder never masks real text.
    assert_eq!(text_display("hello", Some("(untitled)")), ("hello", false));
    // Empty content with a placeholder discloses the stand-in (styled muted
    // by the caller) so an empty Page can never render as a blank row.
    assert_eq!(text_display("", Some("(untitled)")), ("(untitled)", true));
    // No placeholder configured → empty stays empty (never fabricated).
    assert_eq!(text_display("", None), ("", false));
    assert_eq!(text_display("hello", None), ("hello", false));
}

#[test]
fn resolve_states_template_and_default() {
    let row: HashMap<String, Value> = [(
        "sts".to_string(),
        Value::Array(vec![
            Value::String("A".to_string()),
            Value::String("B".to_string()),
        ]),
    )]
    .into_iter()
    .collect();

    let mut args = empty_args();
    args.templates.insert(
        "states".to_string(),
        RenderExpr::ColumnRef {
            name: "sts".to_string(),
        },
    );
    assert_eq!(
        resolve_states(&args, &row, &PlainValueFns).unwrap(),
        vec!["A".to_string(), "B".to_string()]
    );

    let default = resolve_states(&empty_args(), &row, &PlainValueFns).unwrap();
    assert_eq!(
        default,
        vec![
            String::new(),
            "TODO".to_string(),
            "DOING".to_string(),
            "DONE".to_string()
        ]
    );
}

/// When a document has no `#+TODO:` keywords, `todo_states` evaluates
/// to `Value::Null`. `resolve_states` must fall back to the default
/// cycle `["", "TODO", "DOING", "DONE"]` — otherwise a task block in a
/// journal day page (which never declares `#+TODO:`) would cycle from
/// TODO straight to empty, skipping DOING.
#[test]
fn resolve_states_null_value_falls_back_to_default() {
    let row: HashMap<String, Value> = [("sts".to_string(), Value::Null)].into_iter().collect();
    let mut args = empty_args();
    args.templates.insert(
        "states".to_string(),
        RenderExpr::ColumnRef {
            name: "sts".to_string(),
        },
    );
    assert_eq!(
        resolve_states(&args, &row, &PlainValueFns).unwrap(),
        vec![
            String::new(),
            "TODO".to_string(),
            "DOING".to_string(),
            "DONE".to_string()
        ]
    );
}

/// When a document explicitly sets an empty `#+TODO:` list (or the
/// stored JSON is `[]`), `resolve_states` receives an empty Array and
/// must still fall back to the default cycle — otherwise `cycle_state`
/// would return `""` for every click.
#[test]
fn resolve_states_empty_array_falls_back_to_default() {
    let row: HashMap<String, Value> = [("sts".to_string(), Value::Array(vec![]))]
        .into_iter()
        .collect();
    let mut args = empty_args();
    args.templates.insert(
        "states".to_string(),
        RenderExpr::ColumnRef {
            name: "sts".to_string(),
        },
    );
    assert_eq!(
        resolve_states(&args, &row, &PlainValueFns).unwrap(),
        vec![
            String::new(),
            "TODO".to_string(),
            "DOING".to_string(),
            "DONE".to_string()
        ]
    );
}

#[test]
fn drawer_rows_and_column_refs() {
    let drawer_row: Arc<DataRow> = Arc::new(
        [(
            "collapse_to".to_string(),
            Value::String("Drawer".to_string()),
        )]
        .into_iter()
        .collect(),
    );
    let plain_row: Arc<DataRow> = Arc::new(
        [(
            "collapse_to".to_string(),
            Value::String("inline".to_string()),
        )]
        .into_iter()
        .collect(),
    );
    assert!(has_drawer_rows(&[plain_row.clone(), drawer_row]));
    assert!(!has_drawer_rows(&[plain_row]));
    assert!(!has_drawer_rows(&[]));

    let col = RenderExpr::ColumnRef {
        name: "title".to_string(),
    };
    assert_eq!(column_ref_name(&col), Some("title"));
    assert_eq!(column_ref_name(&RenderExpr::Array { items: vec![] }), None);

    let mut args = empty_args();
    assert_eq!(sort_key_column(&args), None);
    args.templates.insert(
        "sort_key".to_string(),
        RenderExpr::ColumnRef {
            name: "seq".to_string(),
        },
    );
    assert_eq!(sort_key_column(&args), Some("seq"));
}

#[test]
fn resolved_args_getters() {
    let mut args = empty_args();
    args.positional = vec![
        Value::Integer(7),
        Value::String("s".to_string()),
        Value::Float(1.25),
        Value::Boolean(true),
        Value::Null,
    ];
    args.positional_exprs = vec![RenderExpr::ColumnRef {
        name: "c0".to_string(),
    }];
    args.named = [
        ("s".to_string(), Value::String("v".to_string())),
        ("f".to_string(), Value::Float(2.5)),
        ("i".to_string(), Value::Integer(3)),
        ("bt".to_string(), Value::Boolean(true)),
        ("bf".to_string(), Value::Boolean(false)),
    ]
    .into_iter()
    .collect();
    args.templates.insert(
        "item_template".to_string(),
        RenderExpr::ColumnRef {
            name: "x".to_string(),
        },
    );

    assert_eq!(args.get_string("s"), Some("v"));
    assert_eq!(args.get_string("missing"), None);
    assert_eq!(args.get_string_or("s", "d"), "v");
    assert_eq!(args.get_string_or("missing", "d"), "d");

    assert_eq!(args.get_f64("f"), Some(2.5));
    assert_eq!(args.get_f64("i"), Some(3.0));
    assert_eq!(args.get_f64("s"), None);
    assert_eq!(args.get_f64("missing"), None);

    assert_eq!(args.get_bool("bt"), Some(true));
    assert_eq!(args.get_bool("bf"), Some(false));
    assert_eq!(args.get_bool("s"), None);

    assert_eq!(args.get_positional_f64(0), Some(7.0));
    assert_eq!(args.get_positional_f64(2), Some(1.25));
    assert_eq!(args.get_positional_f64(9), None);

    assert_eq!(args.get_positional_string(0), Some("7".to_string()));
    assert_eq!(args.get_positional_string(1), Some("s".to_string()));
    assert_eq!(args.get_positional_string(2), Some("1.25".to_string()));
    assert_eq!(args.get_positional_string(3), Some("true".to_string()));
    assert_eq!(args.get_positional_string(4), None);
    assert_eq!(args.get_positional_string(9), None);

    assert_eq!(args.get_positional_column_name(0), Some("c0"));
    assert_eq!(args.get_positional_column_name(5), None);

    assert!(matches!(
        args.get_template("item_template"),
        Some(RenderExpr::ColumnRef { name }) if name == "x"
    ));
    assert!(args.get_rows("nope").is_none());
}

/// The backstop: a name no widget declares and the allowlist does not
/// carry was resolved as a scalar, so `get_template` can only ever answer
/// `None`. That silent `None` is what kept `expand_toggle`'s `content`
/// dead, so it is a panic now rather than an absent-arg answer.
#[test]
#[should_panic(expected = "not a")]
fn get_template_for_an_unclassified_name_panics() {
    let row: HashMap<String, Value> = HashMap::new();
    let args = resolve_args_with(&[], &EvalEnv::of_row(&row), &PlainValueFns).unwrap();
    let _ = args.get_template("nope");
}

/// A widget's own declared params answer templateness — the allowlist is
/// consulted only for names the widget says nothing about.
#[test]
fn declared_params_override_the_global_allowlist() {
    static META: crate::WidgetMeta = crate::WidgetMeta {
        name: "probe",
        category: crate::WidgetCategory::Special,
        params: &[
            crate::StaticParam {
                name: "content",
                type_hint: "Expr",
                default: None,
                slot: None,
            },
            crate::StaticParam {
                name: "header",
                type_hint: "String",
                default: None,
                slot: Some(0),
            },
        ],
        doc: "",
    };
    // Undeclared by `probe`, so the allowlist still rules.
    assert!(is_template_arg_for(Some(&META), "item_template"));
    // Declared as Expr though the allowlist has never heard of it.
    assert!(is_template_arg_for(Some(&META), "content"));
    assert!(!is_template_arg("content"));
    // Declared as a scalar though the allowlist calls it a template.
    assert!(!is_template_arg_for(Some(&META), "header"));
    assert!(is_template_arg("header"));
}

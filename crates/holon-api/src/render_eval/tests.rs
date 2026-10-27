fn eval_binary_op(op: &BinaryOperator, l: &Value, r: &Value) -> Value {
    super::eval_binary_op(op, l, r).unwrap()
}

use super::*;
use crate::render_types::Arg;

#[test]
fn test_eval_binary_op_arithmetic() {
    assert_eq!(
        eval_binary_op(&BinaryOperator::Add, &Value::Integer(2), &Value::Integer(3)),
        Value::Integer(5)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Sub, &Value::Float(5.0), &Value::Float(2.0)),
        Value::Float(3.0)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Mul, &Value::Integer(3), &Value::Integer(4)),
        Value::Integer(12)
    );
    assert_eq!(
        eval_binary_op(
            &BinaryOperator::Div,
            &Value::Integer(10),
            &Value::Integer(3)
        ),
        Value::Integer(3)
    );
}

#[test]
fn test_eval_binary_op_string_concat() {
    assert_eq!(
        eval_binary_op(
            &BinaryOperator::Concat,
            &Value::String("hello ".into()),
            &Value::String("world".into())
        ),
        Value::String("hello world".into())
    );
}

#[test]
fn test_eval_binary_op_comparison() {
    assert_eq!(
        eval_binary_op(&BinaryOperator::Eq, &Value::Integer(1), &Value::Integer(1)),
        Value::Boolean(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Neq, &Value::Integer(1), &Value::Integer(2)),
        Value::Boolean(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Gt, &Value::Integer(3), &Value::Integer(2)),
        Value::Boolean(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Lt, &Value::Float(1.0), &Value::Float(2.0)),
        Value::Boolean(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Gte, &Value::Integer(3), &Value::Integer(3)),
        Value::Boolean(true)
    );
    assert_eq!(
        eval_binary_op(&BinaryOperator::Lte, &Value::Integer(2), &Value::Integer(3)),
        Value::Boolean(true)
    );
}

#[test]
fn test_eval_binary_op_logical() {
    assert_eq!(
        eval_binary_op(
            &BinaryOperator::And,
            &Value::Boolean(true),
            &Value::Boolean(false)
        ),
        Value::Boolean(false)
    );
    assert_eq!(
        eval_binary_op(
            &BinaryOperator::Or,
            &Value::Boolean(false),
            &Value::Boolean(true)
        ),
        Value::Boolean(true)
    );
}

#[test]
fn test_eval_binary_op_missing_in_missing_out() {
    for op in [
        BinaryOperator::Add,
        BinaryOperator::Mul,
        BinaryOperator::Concat,
    ] {
        assert_eq!(
            eval_binary_op(&op, &Value::Null, &Value::Integer(1)),
            Value::Null,
            "{op:?}"
        );
    }
}

#[test]
fn test_eval_binary_op_wrongly_typed_operands_are_errors() {
    let cases = [
        (BinaryOperator::Add, Value::Integer(1), Value::Boolean(true)),
        (
            BinaryOperator::Gt,
            Value::String("a".into()),
            Value::String("b".into()),
        ),
        (BinaryOperator::And, Value::Integer(1), Value::Integer(0)),
    ];
    for (op, l, r) in cases {
        assert!(
            super::eval_binary_op(&op, &l, &r).is_err(),
            "{op:?} on {l:?}, {r:?} must be an error"
        );
    }
}

#[test]
fn test_if_with_a_missing_condition_takes_otherwise() {
    let then = RenderExpr::Literal {
        value: Value::String("then".into()),
    };
    let otherwise = RenderExpr::Literal {
        value: Value::String("otherwise".into()),
    };
    assert_eq!(
        choose_branch(&Value::Null, &then, &otherwise).unwrap(),
        &otherwise
    );
    assert_eq!(
        choose_branch(&Value::Boolean(true), &then, &otherwise).unwrap(),
        &then
    );
    assert!(choose_branch(&Value::Integer(1), &then, &otherwise).is_err());
}

#[test]
fn test_eval_plain_value_literal() {
    let row = crate::StorageEntity::new();
    let expr = RenderExpr::Literal {
        value: Value::Integer(42),
    };
    assert_eq!(eval_plain_value(&expr, &row).unwrap(), Value::Integer(42));
}

#[test]
fn test_eval_plain_value_column_ref() {
    let mut row = crate::StorageEntity::new();
    row.insert("name".into(), Value::String("Alice".into()));
    let expr = RenderExpr::ColumnRef {
        name: "name".to_string(),
    };
    assert_eq!(
        eval_plain_value(&expr, &row).unwrap(),
        Value::String("Alice".into())
    );
}

#[test]
fn test_eval_plain_value_missing_column() {
    let row = crate::StorageEntity::new();
    let expr = RenderExpr::ColumnRef {
        name: "missing".to_string(),
    };
    assert_eq!(eval_plain_value(&expr, &row).unwrap(), Value::Null);
}

#[test]
fn test_eval_plain_value_binary_op() {
    let row = crate::StorageEntity::new();
    let expr = RenderExpr::BinaryOp {
        op: BinaryOperator::Add,
        left: Box::new(RenderExpr::Literal {
            value: Value::Integer(1),
        }),
        right: Box::new(RenderExpr::Literal {
            value: Value::Integer(2),
        }),
    };
    assert_eq!(eval_plain_value(&expr, &row).unwrap(), Value::Integer(3));
}

#[test]
fn test_eval_plain_value_concat() {
    let row = crate::StorageEntity::new();
    let expr = RenderExpr::FunctionCall {
        name: "concat".to_string(),
        args: vec![
            Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::String("hello".into()),
                },
            },
            Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::String(" world".into()),
                },
            },
        ],
    };
    assert_eq!(
        eval_plain_value(&expr, &row).unwrap(),
        Value::String("hello world".into())
    );
}

#[test]
fn test_eval_plain_value_array() {
    let row = crate::StorageEntity::new();
    let expr = RenderExpr::Array {
        items: vec![
            RenderExpr::Literal {
                value: Value::Integer(1),
            },
            RenderExpr::Literal {
                value: Value::Integer(2),
            },
        ],
    };
    assert_eq!(
        eval_plain_value(&expr, &row).unwrap(),
        Value::Array(vec![Value::Integer(1), Value::Integer(2)])
    );
}

#[test]
fn test_resolve_args_named_and_positional() {
    let mut row = crate::StorageEntity::new();
    row.insert("col1".into(), Value::String("val1".into()));

    let args = vec![
        Arg {
            name: None,
            value: RenderExpr::ColumnRef {
                name: "col1".to_string(),
            },
        },
        Arg {
            name: Some("title".to_string()),
            value: RenderExpr::Literal {
                value: Value::String("My Title".into()),
            },
        },
        Arg {
            name: Some("item_template".to_string()),
            value: RenderExpr::Literal { value: Value::Null },
        },
    ];

    let resolved = resolve_args_with(&args, &EvalEnv::of_row(&row), &PlainValueFns).unwrap();
    assert_eq!(resolved.positional.len(), 1);
    assert_eq!(resolved.positional[0], Value::String("val1".into()));
    assert_eq!(
        resolved.named.get("title"),
        Some(&Value::String("My Title".into()))
    );
    assert!(resolved.templates.contains_key("item_template"));
    assert_eq!(resolved.get_positional_column_name(0), Some("col1"));
}

#[test]
fn test_is_template_arg() {
    assert!(is_template_arg("item_template"));
    assert!(is_template_arg("item"));
    assert!(is_template_arg("header"));
    assert!(is_template_arg("states"));
    assert!(!is_template_arg("title"));
    assert!(!is_template_arg("width"));
}

#[test]
fn test_to_display_string() {
    assert_eq!(Value::String("hello".into()).to_display_string(), "hello");
    assert_eq!(Value::Integer(42).to_display_string(), "42");
    assert_eq!(Value::Float(2.5).to_display_string(), "2.5");
    assert_eq!(Value::Boolean(true).to_display_string(), "true");
    assert_eq!(Value::Null.to_display_string(), "");
    assert_eq!(
        Value::Array(vec![Value::Integer(1), Value::Integer(2)]).to_display_string(),
        "1, 2"
    );
}

#[test]
fn test_sorted_rows() {
    let rows: Vec<Arc<DataRow>> = vec![
        Arc::new(HashMap::from([
            ("name".into(), Value::String("b".into())),
            ("sort".into(), Value::Integer(2)),
        ])),
        Arc::new(HashMap::from([
            ("name".into(), Value::String("a".into())),
            ("sort".into(), Value::Integer(1)),
        ])),
        Arc::new(HashMap::from([
            ("name".into(), Value::String("c".into())),
            ("sort".into(), Value::Integer(3)),
        ])),
    ];
    let sorted = sorted_rows(&rows, Some("sort"));
    assert_eq!(sorted[0].get("name"), Some(&Value::String("a".into())));
    assert_eq!(sorted[2].get("name"), Some(&Value::String("c".into())));

    let unsorted = sorted_rows(&rows, None);
    assert_eq!(unsorted[0].get("name"), Some(&Value::String("b".into())));
}

#[test]
fn test_outline_tree() {
    let rows: Vec<Arc<DataRow>> = vec![
        Arc::new(HashMap::from([
            ("id".into(), Value::String("1".into())),
            ("parent_id".into(), Value::String("root".into())),
            ("sort_key".into(), Value::Integer(1)),
        ])),
        Arc::new(HashMap::from([
            ("id".into(), Value::String("2".into())),
            ("parent_id".into(), Value::String("1".into())),
            ("sort_key".into(), Value::Integer(1)),
        ])),
        Arc::new(HashMap::from([
            ("id".into(), Value::String("3".into())),
            ("parent_id".into(), Value::String("root".into())),
            ("sort_key".into(), Value::Integer(2)),
        ])),
    ];

    let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key", None);
    assert_eq!(tree.roots.len(), 2);

    let items: Vec<(String, usize)> = tree.walk_depth_first(|row, depth| {
        let id = row.get("id").unwrap().as_string().unwrap().to_string();
        (id, depth)
    });
    assert_eq!(
        items,
        vec![
            ("1".to_string(), 0),
            ("2".to_string(), 1),
            ("3".to_string(), 0),
        ]
    );
}

// ── RULING C1' — per-level tree sort keys ──────────────────────────
//
// Helper: DFS the tree, collecting `id`s in render order.
fn tree_ids(tree: &OutlineTree) -> Vec<String> {
    tree.walk_depth_first(|row, _| row.get("id").unwrap().as_string().unwrap().to_string())
}

fn row(pairs: &[(&str, Value)]) -> Arc<DataRow> {
    Arc::new(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

/// The C1' RED→GREEN core: two ROOT pins whose `added_ts` (pin recency) is
/// REVERSED vs `sort_key` (ingest order). With a declared root key
/// `"-added_ts"`, roots must render most-recently-pinned-first
/// (`[apple, zebra]`), while `apple`'s CHILDREN keep `sort_key` ASC — the
/// per-level split. Pre-C1' (root key ignored) roots would follow
/// `sort_key` → `[zebra, apple]`, so this reds until the per-level re-sort
/// lands.
#[test]
fn from_rows_roots_sort_by_root_key_children_keep_sort_key() {
    let s = |x: &str| Value::String(x.into());
    let i = |n: i64| Value::Integer(n);
    let rows: Vec<Arc<DataRow>> = vec![
        // zebra: ingested 1st (sort_key=1), pinned 1st (added_ts=10)
        row(&[
            ("id", s("zebra")),
            ("parent_id", s("root")),
            ("sort_key", i(1)),
            ("added_ts", i(10)),
        ]),
        // apple: ingested 2nd (sort_key=2), pinned last (added_ts=20)
        row(&[
            ("id", s("apple")),
            ("parent_id", s("root")),
            ("sort_key", i(2)),
            ("added_ts", i(20)),
        ]),
        // apple's children, supplied OUT of sort_key order in the input.
        row(&[
            ("id", s("apple-c2")),
            ("parent_id", s("apple")),
            ("sort_key", i(2)),
            ("added_ts", i(99)),
        ]),
        row(&[
            ("id", s("apple-c1")),
            ("parent_id", s("apple")),
            ("sort_key", i(1)),
            ("added_ts", i(5)),
        ]),
    ];
    let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key", Some("-added_ts"));
    assert_eq!(
        tree_ids(&tree),
        vec![
            "apple".to_string(),    // root, added_ts DESC → first
            "apple-c1".to_string(), // child, sort_key ASC
            "apple-c2".to_string(),
            "zebra".to_string(), // root, added_ts DESC → last
        ],
        "roots must sort by the declared root key (added_ts DESC) while child \
             buckets keep sort_key (document) order",
    );
}

/// No root key declared → roots stay in `sort_key` order, byte-identical to
/// the pre-C1' behavior. Locks the "no global flip" guarantee.
#[test]
fn from_rows_none_root_key_keeps_sort_key_order_exactly() {
    let s = |x: &str| Value::String(x.into());
    let i = |n: i64| Value::Integer(n);
    let rows: Vec<Arc<DataRow>> = vec![
        row(&[
            ("id", s("zebra")),
            ("parent_id", s("root")),
            ("sort_key", i(1)),
            ("added_ts", i(10)),
        ]),
        row(&[
            ("id", s("apple")),
            ("parent_id", s("root")),
            ("sort_key", i(2)),
            ("added_ts", i(20)),
        ]),
    ];
    let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key", None);
    assert_eq!(
        tree_ids(&tree),
        vec!["zebra".to_string(), "apple".to_string()],
        "with no root key, roots keep sort_key order (pre-C1' behavior)",
    );
}

/// Roots that TIE on the root key fall back to `sort_key` order
/// deterministically (the re-sort is stable over the `sort_col`-ordered
/// bucket). `zebra` (sort_key=1) precedes `apple` (sort_key=2) despite both
/// having `added_ts=10`.
#[test]
fn from_rows_root_key_ties_fall_back_to_sort_key_deterministically() {
    let s = |x: &str| Value::String(x.into());
    let i = |n: i64| Value::Integer(n);
    let rows: Vec<Arc<DataRow>> = vec![
        row(&[
            ("id", s("apple")),
            ("parent_id", s("root")),
            ("sort_key", i(2)),
            ("added_ts", i(10)),
        ]),
        row(&[
            ("id", s("zebra")),
            ("parent_id", s("root")),
            ("sort_key", i(1)),
            ("added_ts", i(10)),
        ]),
    ];
    let tree = OutlineTree::from_rows(&rows, "parent_id", "sort_key", Some("-added_ts"));
    assert_eq!(
        tree_ids(&tree),
        vec!["zebra".to_string(), "apple".to_string()],
        "root-key ties break deterministically on sort_key order",
    );
}

#[test]
fn test_partition_screen_columns() {
    let rows: Vec<Arc<DataRow>> = vec![
        Arc::new(HashMap::from([
            ("name".into(), Value::String("left".into())),
            ("collapse_to".into(), Value::String("drawer".into())),
        ])),
        Arc::new(HashMap::from([(
            "name".into(),
            Value::String("main".into()),
        )])),
        Arc::new(HashMap::from([
            ("name".into(), Value::String("right".into())),
            ("collapse_to".into(), Value::String("drawer".into())),
        ])),
    ];
    let p = partition_screen_columns(&rows, |row| {
        row.get("name").unwrap().as_string().unwrap().to_string()
    });
    assert_eq!(
        p.left_sidebar.as_ref().map(|r| r.widget.as_str()),
        Some("left")
    );
    assert_eq!(
        p.right_sidebar.as_ref().map(|r| r.widget.as_str()),
        Some("right")
    );
    assert_eq!(p.main.len(), 1);
    assert_eq!(p.main[0].widget, "main");
}

#[test]
fn test_cycle_state() {
    let states = vec!["".into(), "TODO".into(), "DOING".into(), "DONE".into()];
    assert_eq!(cycle_state("", &states), "TODO");
    assert_eq!(cycle_state("TODO", &states), "DOING");
    assert_eq!(cycle_state("DONE", &states), "");
}

/// The LogSeq ring is NOT in `states`: a block in `LATER` or `NOW` must
/// keep walking LATER -> NOW -> DONE even though the native list it is
/// cycled against holds neither keyword. Only the keystone exercised this
/// before, and it walks the ring by calling `cycle_state` itself, so both
/// arms could be deleted without a test going red.
#[test]
fn the_logseq_ring_survives_the_native_keyword_list() {
    let native = vec!["".into(), "TODO".into(), "DOING".into(), "DONE".into()];
    assert_eq!(cycle_state("LATER", &native), "NOW");
    assert_eq!(cycle_state("NOW", &native), "DONE");
    // DONE re-enters the native ring (ForeignVaultCompat §4).
    assert_eq!(cycle_state("DONE", &native), "");
}

#[test]
fn test_state_display() {
    assert_eq!(state_display("TODO"), ("TODO", ThemeToken::Muted));
    assert_eq!(state_display("DOING"), ("DOING", ThemeToken::Warning));
    assert_eq!(state_display("DONE"), ("[x]", ThemeToken::Success));
    assert_eq!(state_display(""), ("", ThemeToken::Muted));
    assert_eq!(state_display("CUSTOM"), ("CUSTOM", ThemeToken::Primary));
}

#[test]
fn an_unknown_call_in_a_plain_value_is_refused_naming_it() {
    let row = crate::StorageEntity::new();
    let expr = RenderExpr::FunctionCall {
        name: "definitely_not_registered".to_string(),
        args: vec![Arg {
            name: None,
            value: RenderExpr::Literal {
                value: Value::Integer(7),
            },
        }],
    };
    assert_eq!(
        eval_plain_value(&expr, &row),
        Err(ComputeError::NotAPlainValueFunction {
            name: "definitely_not_registered".to_string(),
            call: "definitely_not_registered(7)".to_string(),
        })
    );
}

#[test]
fn core_concat_still_works() {
    let row = crate::StorageEntity::new();
    let expr = RenderExpr::FunctionCall {
        name: "concat".to_string(),
        args: vec![
            Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::String("ab".into()),
                },
            },
            Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::String("cd".into()),
                },
            },
        ],
    };
    assert_eq!(
        eval_plain_value(&expr, &row).unwrap(),
        Value::String("abcd".into())
    );
}

// ── Value-fn dispatch via resolve_args_with / eval_to_interp ───────

struct MockValueFnLookup;
impl ValueFnLookup for MockValueFnLookup {
    fn call_kind(&self, name: &str) -> Option<CallKind> {
        (name == "echo").then_some(CallKind::ValueFn)
    }

    fn invoke(&self, name: &str, args: &ResolvedArgs) -> Option<Result<InterpValue, ComputeError>> {
        match name {
            "echo" => args
                .positional
                .first()
                .cloned()
                .map(|v| Ok(InterpValue::Value(v))),
            _ => None,
        }
    }

    fn unknown_function(&self, name: &str, call: String) -> ComputeError {
        ComputeError::UnknownFunction {
            name: name.to_string(),
            call,
        }
    }
}

#[test]
fn registered_value_fn_dispatches() {
    let row = crate::StorageEntity::new();
    let expr = RenderExpr::FunctionCall {
        name: "echo".to_string(),
        args: vec![Arg {
            name: None,
            value: RenderExpr::Literal {
                value: Value::Integer(99),
            },
        }],
    };
    match eval_to_interp(&expr, &EvalEnv::of_row(&row), &MockValueFnLookup).unwrap() {
        InterpValue::Value(v) => assert_eq!(v, Value::Integer(99)),
        InterpValue::Rows(_) => panic!("expected Value"),
    }
}

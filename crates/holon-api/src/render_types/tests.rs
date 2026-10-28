use super::*;

fn fc(name: &str, args: Vec<Arg>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.into(),
        args,
    }
}

fn named_arg(name: &str, value: RenderExpr) -> Arg {
    Arg {
        name: Some(name.into()),
        value,
    }
}

#[test]
fn every_type_hint_round_trips_through_json() {
    let block = EntityName::new("block");
    let hints = [
        TypeHint::Bool,
        TypeHint::String,
        TypeHint::Number,
        TypeHint::EntityId {
            entity_name: block.clone(),
        },
        TypeHint::EntityIdOrRoot {
            entity_name: block.clone(),
        },
        TypeHint::RowKey,
        TypeHint::FieldValue {
            field_param: "field".into(),
            fields: vec![OperationParam {
                name: "parent_id".into(),
                type_hint: TypeHint::EntityIdOrRoot {
                    entity_name: block.clone(),
                },
                description: String::new(),
            }],
        },
        TypeHint::Enum {
            type_name: "TaskState".into(),
            variants: vec!["TODO".into()],
        },
        TypeHint::Object {
            fields: vec![OperationParam {
                name: "inner".into(),
                type_hint: TypeHint::RowKey,
                description: String::new(),
            }],
        },
        TypeHint::Expr,
        TypeHint::Collection,
        TypeHint::Newtype {
            type_name: "FileName".into(),
        },
        TypeHint::PlaceId {
            place: crate::types::PlaceName("Project".into()),
        },
    ];
    // No `_` arm: a new variant does not compile until it gets an index here
    // and a case in `hints`.
    const VARIANTS: usize = 13;
    let index = |h: &TypeHint| match h {
        TypeHint::Bool => 0,
        TypeHint::String => 1,
        TypeHint::Number => 2,
        TypeHint::EntityId { .. } => 3,
        TypeHint::EntityIdOrRoot { .. } => 4,
        TypeHint::FieldValue { .. } => 5,
        TypeHint::RowKey => 6,
        TypeHint::Object { .. } => 7,
        TypeHint::Expr => 8,
        TypeHint::Collection => 9,
        TypeHint::Enum { .. } => 10,
        TypeHint::Newtype { .. } => 11,
        TypeHint::PlaceId { .. } => 12,
    };
    let covered: std::collections::BTreeSet<usize> = hints.iter().map(index).collect();
    assert_eq!(
        covered,
        (0..VARIANTS).collect(),
        "a TypeHint variant has no round-trip case"
    );
    for type_hint in hints {
        let param = OperationParam {
            name: "p".into(),
            type_hint,
            description: String::new(),
        };
        let json = serde_json::to_string(&param).expect("serialize OperationParam");
        let back: OperationParam = serde_json::from_str(&json)
            .unwrap_or_else(|e| panic!("{json} did not deserialize: {e}"));
        assert_eq!(back, param);
    }
}

#[test]
fn to_rhai_table() {
    assert_eq!(fc("table", vec![]).to_rhai(), "table()");
}

#[test]
fn to_rhai_list_with_live_block() {
    let expr = fc(
        "list",
        vec![named_arg("item_template", fc("live_block", vec![]))],
    );
    assert_eq!(expr.to_rhai(), r#"list(#{item_template: live_block()})"#);
}

#[test]
fn to_rhai_columns_with_gap() {
    let expr = fc(
        "columns",
        vec![
            named_arg(
                "gap",
                RenderExpr::Literal {
                    value: Value::Integer(4),
                },
            ),
            named_arg("item_template", fc("live_block", vec![])),
        ],
    );
    assert_eq!(
        expr.to_rhai(),
        r#"columns(#{gap: 4, item_template: live_block()})"#,
    );
}

#[test]
fn to_rhai_nested_row_text_col() {
    let expr = fc(
        "list",
        vec![named_arg(
            "item_template",
            fc(
                "row",
                vec![Arg {
                    name: None,
                    value: fc(
                        "text",
                        vec![Arg {
                            name: None,
                            value: RenderExpr::ColumnRef {
                                name: "content".into(),
                            },
                        }],
                    ),
                }],
            ),
        )],
    );
    assert_eq!(
        expr.to_rhai(),
        r#"list(#{item_template: row(text(col("content")))})"#,
    );
}

#[test]
fn to_rhai_column_ref() {
    assert_eq!(
        RenderExpr::ColumnRef { name: "id".into() }.to_rhai(),
        r#"col("id")"#,
    );
}

#[test]
fn visible_columns_extracts_column_refs() {
    let expr = fc(
        "row",
        vec![
            Arg {
                name: None,
                value: fc(
                    "text",
                    vec![Arg {
                        name: None,
                        value: RenderExpr::ColumnRef {
                            name: "content".into(),
                        },
                    }],
                ),
            },
            Arg {
                name: None,
                value: fc(
                    "badge",
                    vec![Arg {
                        name: None,
                        value: RenderExpr::ColumnRef {
                            name: "task_state".into(),
                        },
                    }],
                ),
            },
        ],
    );
    let cols = expr.visible_columns();
    assert_eq!(cols, vec!["content", "task_state"]);
}

#[test]
fn visible_columns_empty_for_live_block() {
    let expr = fc(
        "list",
        vec![named_arg("item_template", fc("live_block", vec![]))],
    );
    assert!(expr.visible_columns().is_empty());
}

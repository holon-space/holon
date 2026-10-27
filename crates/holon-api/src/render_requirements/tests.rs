use super::*;
use crate::Value;
use crate::render_types::Arg;

fn call(name: &str, args: Vec<Arg>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args,
    }
}

fn col(name: &str) -> RenderExpr {
    call(
        "col",
        vec![Arg {
            name: None,
            value: RenderExpr::Literal {
                value: Value::String(name.to_string()),
            },
        }],
    )
}

fn register() {
    register_widget_param_defaults(vec![
        (
            "icon".to_string(),
            vec![
                ("name".to_string(), true),
                ("size".to_string(), true),
                ("color".to_string(), true),
            ],
        ),
        (
            "text".to_string(),
            vec![("content".to_string(), false), ("style".to_string(), true)],
        ),
    ]);
}

#[test]
fn a_defaulting_parameter_makes_its_binding_optional() {
    register();
    let names = bound_names(&call(
        "row",
        vec![
            Arg {
                name: None,
                value: call(
                    "icon",
                    vec![Arg {
                        name: None,
                        value: col("bullet_shape"),
                    }],
                ),
            },
            Arg {
                name: None,
                value: call(
                    "text",
                    vec![Arg {
                        name: None,
                        value: col("content"),
                    }],
                ),
            },
        ],
    ));
    assert!(
        names.optional.contains("bullet_shape"),
        "`icon`'s first parameter declares a default, so its binding degrades: {names:?}"
    );
    assert!(
        names.required.contains("content"),
        "`text`'s first parameter declares no default: {names:?}"
    );
}

#[test]
fn a_computed_binding_expands_to_the_columns_beneath_it() {
    let mut computed = BTreeMap::new();
    computed.insert(
        "bullet_shape".to_string(),
        BTreeSet::from(["collapsed".to_string()]),
    );
    computed.insert(
        "is_program".to_string(),
        BTreeSet::from(["is_rule_head".to_string(), "parent_id".to_string()]),
    );
    computed.insert(
        "is_rule_head".to_string(),
        BTreeSet::from(["source_language".to_string()]),
    );

    let names = BoundNames {
        required: BTreeSet::from(["is_program".to_string()]),
        optional: BTreeSet::from(["bullet_shape".to_string()]),
        dispatches_to_entity: false,
    };
    let reqs = expand_through_computed(&names, &computed);
    assert_eq!(
        reqs.required(),
        &BTreeSet::from(["parent_id".to_string(), "source_language".to_string()]),
        "transitive through the sibling computed field"
    );
    assert_eq!(reqs.optional(), &BTreeSet::from(["collapsed".to_string()]));
}

#[test]
fn required_beats_optional_when_two_renderers_bind_the_same_column() {
    let a =
        RenderRequirements::from_parts(BTreeSet::new(), BTreeSet::from(["collapsed".to_string()]));
    let b =
        RenderRequirements::from_parts(BTreeSet::from(["collapsed".to_string()]), BTreeSet::new());
    let merged = a.union(&b);
    assert!(merged.requires("collapsed"));
    assert!(!merged.optional().contains("collapsed"));
}

#[test]
fn unmet_by_names_only_the_required_columns_a_projection_drops() {
    let reqs = RenderRequirements::from_parts(
        BTreeSet::from(["content_type".to_string(), "widget_only".to_string()]),
        BTreeSet::from(["collapsed".to_string()]),
    );
    assert_eq!(
        reqs.unmet_by(vec!["id", "content", "content_type"]),
        vec!["widget_only".to_string()],
    );
}

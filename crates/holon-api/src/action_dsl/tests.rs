use super::*;
use crate::render_types::RenderExpr;

#[test]
fn an_action_parses_into_entity_operation_and_named_params() {
    let parsed = parse_action_dsl(r#"block.create(#{ content: "hi" })"#).unwrap();
    assert_eq!(parsed.entity, "block");
    assert_eq!(parsed.operation, "create");
    assert_eq!(parsed.params.len(), 1);
    assert_eq!(parsed.params[0].name.as_deref(), Some("content"));

    let parsed = parse_action_dsl(r#"block.set_field(#{ done: true })"#).unwrap();
    assert_eq!(parsed.operation, "set_field");
}

#[test]
fn a_param_over_col_stays_a_per_row_expression() {
    let parsed =
        parse_action_dsl(r#"block.create(#{ content: "Re: " + col("title"), n: col("n") * 2 })"#)
            .unwrap();
    let rhai: Vec<String> = parsed.params.iter().map(|a| a.value.to_rhai()).collect();
    assert!(
        parsed
            .params
            .iter()
            .all(|a| !matches!(a.value, RenderExpr::Literal { .. })),
        "params over col(..) must not be decided at load; got {rhai:?}"
    );
}

#[test]
fn an_unknown_operation_or_a_non_per_row_param_is_refused() {
    for source in [
        r#"block.explode(#{})"#,
        r#"page.create(#{})"#,
        r#"block.create("x")"#,
        r#"block.create(#{ content: col("a").len() })"#,
    ] {
        assert!(
            parse_action_dsl(source).is_err(),
            "{source} must be refused"
        );
    }
}

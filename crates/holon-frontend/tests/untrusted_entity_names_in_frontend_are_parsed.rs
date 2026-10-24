//! An entity name the frontend takes from an authored action or from a data
//! row is untrusted: it must be refused or parsed, never built raw.

use std::sync::Arc;

use holon_api::EntityName;
use holon_api::Value;
use holon_api::render_types::RenderExpr;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::ViewModel;

#[test]
fn an_action_whose_entity_names_no_entity_is_refused() {
    let action = RenderExpr::FunctionCall {
        name: "has space.set_field".to_string(),
        args: vec![],
    };
    let ctx = RenderContext::for_row(Arc::new(DataRow::new()));
    let err =
        holon_frontend::operations::parse_action_expr(&action, &StubBuilderServices::new(), &ctx)
            .expect_err("`has space` names no entity");
    assert!(
        err.to_string().contains("is not a valid entity name"),
        "{err}"
    );
}

#[test]
fn a_row_entity_name_is_a_parsed_entity_name() {
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String("has space:1".to_string()));
    let node = ViewModel::leaf("text", Value::Null).with_entity(Arc::new(row));
    let name = node.entity_name();
    assert!(
        name.as_ref()
            .is_none_or(|n| EntityName::parse_named(n.as_str()).as_ref() == Ok(n)),
        "a row id `has space:1` gave the entity name {name:?}"
    );
}

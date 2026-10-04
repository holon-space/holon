use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;

fn named(name: &str, value: RenderExpr) -> Arg {
    Arg {
        name: Some(name.to_string()),
        value,
    }
}

fn literal(value: Value) -> RenderExpr {
    RenderExpr::Literal { value }
}

#[test]
fn a_mode_template_with_no_json_form_paints_an_error_node_naming_it() {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let switcher = RenderExpr::FunctionCall {
        name: "view_mode_switcher".to_string(),
        args: vec![
            named("entity_uri", literal(Value::String("block:x".into()))),
            named("modes", literal(Value::String(r#"[{"name":"a"}]"#.into()))),
            named(
                "mode_a",
                RenderExpr::FunctionCall {
                    name: "text".to_string(),
                    args: vec![Arg {
                        name: None,
                        value: literal(Value::Float(f64::INFINITY)),
                    }],
                },
            ),
        ],
    };

    let vm = StubBuilderServices::new().interpret(&switcher, &RenderContext::default());

    assert_eq!(vm.widget_name().as_deref(), Some("error"));
    let message = vm.prop_str("message").unwrap();
    assert!(message.contains("mode_a"), "{message}");
    assert!(message.contains("Value::Float inf"), "{message}");
}

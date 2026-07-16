use holon_api::render_types::RenderExpr;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;

fn interpret(expr: &RenderExpr) -> ReactiveViewModel {
    let services = StubBuilderServices::new();
    let ctx = RenderContext::default();
    services.interpret(expr, &ctx)
}

#[test]
fn divider_is_parsed_and_interpreted() {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();

    let dsl = "divider()";
    let parsed = holon_api::render_dsl::parse_render_dsl(dsl).expect("divider() should parse");

    let vm = interpret(&parsed);
    let name = vm.widget_name();
    assert_eq!(name.as_deref(), Some("divider"));
}

#[test]
fn divider_has_no_props() {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();

    let dsl = "divider()";
    let parsed = holon_api::render_dsl::parse_render_dsl(dsl).expect("divider() should parse");

    let vm = interpret(&parsed);
    assert_eq!(vm.widget_name().as_deref(), Some("divider"));

    // No props expected for a parameterless divider.
    // All supported keys should return None.
    assert!(vm.prop_str("width").is_none());
    assert!(vm.prop_str("height").is_none());
    assert!(vm.prop_str("color").is_none());
}

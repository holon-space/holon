//! Regression: the `text(..., #{style: "h1"})` kwarg must survive
//! interpretation.
//!
//! The page-title block variant renders `text(col("content"), #{style: "h1"})`.
//! Before `style` was a declared param on the `text` widget builder, the macro
//! extracted only `content`/`bold`/`size`/`color` and the `style` kwarg was
//! silently dropped — so the title rendered at the 14px body default instead of
//! the heading scale. This test locks the kwarg onto the resulting ViewModel's
//! props so the drop cannot silently return.

use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;

fn text_with_style(content: &str, style: &str) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "text".to_string(),
        args: vec![
            Arg {
                name: None,
                value: RenderExpr::Literal {
                    value: Value::String(content.to_string()),
                },
            },
            Arg {
                name: Some("style".to_string()),
                value: RenderExpr::Literal {
                    value: Value::String(style.to_string()),
                },
            },
        ],
    }
}

#[test]
fn text_style_kwarg_is_preserved_as_a_prop() {
    let services = StubBuilderServices::new();
    let ctx = RenderContext::default();
    let vm = services.interpret(&text_with_style("Journals", "h1"), &ctx);

    assert_eq!(vm.widget_name().as_deref(), Some("text"));
    assert_eq!(
        vm.prop_str("style").as_deref(),
        Some("h1"),
        "the `style` kwarg must reach the widget props, not be silently dropped"
    );
    // The heading scale itself is resolved from this prop at render time; the
    // shared mapping is asserted in holon-api render_eval tests.
    assert!(
        holon_api::render_eval::text_style_font_size("h1").unwrap() > 14.0,
        "h1 must resolve above body size"
    );
}

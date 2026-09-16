//! A colour name in a layout doc must be refused at the DSL build boundary.
//!
//! Every colour the render DSL accepts is a plain string prop, and nothing
//! validated it. `text(..., #{color: "corrigendum"})` built a `text` widget
//! that carried the nonsense name, and the frontend's colour resolver only saw
//! it inside the frame loop, where the only options are a panic or a silent
//! fallback. A typo therefore painted `foreground` and said nothing.
//!
//! The refusal belongs here, at the same boundary that already refuses a bad
//! `live_query(source: ...)`, so the error widget carries the message and
//! `inv-frontend-no-error-widgets` can see it. `holon_api::theme_token` owns
//! the vocabulary; these cases pin that each colour-accepting builder uses it.
//!
//! The `threshold` case is the one that keeps the token set from being a
//! per-widget guess: `threshold` is not a token, so `icon` must refuse it, and
//! `primary` is a token for `icon` and for `text` alike, because the vocabulary
//! is shared rather than re-derived per widget.

use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;

fn literal(text: &str) -> RenderExpr {
    RenderExpr::Literal {
        value: Value::String(text.to_string()),
    }
}

fn named_call(name: &str, prop: &str, value: &str) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args: vec![
            Arg {
                name: None,
                value: literal("label"),
            },
            Arg {
                name: Some(prop.to_string()),
                value: literal(value),
            },
        ],
    }
}

fn interpret(expr: &RenderExpr) -> holon_frontend::reactive_view_model::ReactiveViewModel {
    StubBuilderServices::new().interpret(expr, &RenderContext::default())
}

/// The refusal, and the message that makes it actionable.
#[test]
fn an_unknown_colour_name_is_refused_at_the_build_boundary() {
    let vm = interpret(&named_call("text", "color", "corrigendum"));

    assert_eq!(
        vm.widget_name().as_deref(),
        Some("error"),
        "a colour name no theme defines must be refused when the layout doc is built, not \
         carried to the frame loop where the only options are a panic or a silent fallback. \
         Got widget {:?} with props {:?}",
        vm.widget_name(),
        vm.prop_str("color"),
    );
    let message = vm.prop_str("message").unwrap_or_default();
    assert!(
        message.contains("corrigendum"),
        "the refusal must name the offending value so the author can fix it, got {message:?}"
    );
}

/// A valid token survives as the prop the renderer reads.
#[test]
fn a_known_colour_name_is_carried_through() {
    let vm = interpret(&named_call("text", "color", "muted"));

    assert_eq!(vm.widget_name().as_deref(), Some("text"));
    assert_eq!(
        vm.prop_str("color").as_deref(),
        Some("muted"),
        "a known token must reach the renderer unchanged"
    );
}

/// A valid name survives as the SAME string. The validation must not rewrite:
/// a props-only widget's fast path re-reads the colour from the expression
/// without passing through this builder, so a rewrite here would have the two
/// paths write different values into one prop.
#[test]
fn a_valid_name_is_carried_through_unrewritten() {
    for name in ["muted", "secondary", "primary"] {
        let vm = interpret(&named_call("text", "color", name));
        assert_eq!(vm.widget_name().as_deref(), Some("text"));
        assert_eq!(
            vm.prop_str("color").as_deref(),
            Some(name),
            "a known token must reach the renderer unchanged"
        );
    }
}

/// The vocabulary is shared, not re-derived per widget. `primary` is a token
/// for `icon` today and for `text` it was silently `foreground`; both must
/// accept it, and both must refuse a name that is not in the table.
#[test]
fn the_vocabulary_is_shared_across_widgets() {
    for (widget, prop) in [
        ("text", "color"),
        ("icon", "color"),
        ("spacer", "color"),
        ("card", "accent"),
    ] {
        let refused = interpret(&named_call(widget, prop, "threshold"));
        assert_eq!(
            refused.widget_name().as_deref(),
            Some("error"),
            "{widget}(#{{{prop}: \"threshold\"}}) must be refused: `threshold` is not in the \
             token table. Got {:?}",
            refused.widget_name(),
        );

        let accepted = interpret(&named_call(widget, prop, "primary"));
        assert_ne!(
            accepted.widget_name().as_deref(),
            Some("error"),
            "{widget}(#{{{prop}: \"primary\"}}) must be accepted: `primary` is in the token \
             table. Got error {:?}",
            accepted.prop_str("message"),
        );
    }
}

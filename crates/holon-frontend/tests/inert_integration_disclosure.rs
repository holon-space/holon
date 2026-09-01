//! Contract: the disclosure a view over a not-connected integration renders
//! must survive to the layer every frontend and every headless oracle reads.
//!
//! The first attempt used a bespoke `degraded` widget kind, with its own
//! shadow and GPUI builders. Those builders made it render on GPUI, which is
//! what hid the bug: `builder_registry!` registered the name wholesale, so the
//! interpreter built a real node with its props intact. The break was one
//! layer down — `to_view_kind` has no `"degraded"` arm, so the STATIC
//! `ViewModel` became `ViewKind::Empty`, `widget_name()` returned `None`,
//! dioxus-web's static-`ViewModel` dispatch took its `empty` arm and painted
//! nothing, and the headless snapshot read `"empty"` with no message. The name
//! was also absent from `TUI_SUPPORTED_WIDGETS`.
//!
//! Both builders are now deleted, so `"degraded"` is merely UNREGISTERED, and
//! an unregistered name fails a third way, pinned below: the interpreter
//! substitutes the placeholder text `[unknown: degraded]` and drops the props
//! entirely. Either route loses the sentence; only the shipped one was the
//! `ViewKind::Empty` route above.
//!
//! These pin the shape
//! `crates/holon/src/api/ui_watcher.rs::disclosed_render_expr` emits: a widget
//! kind every frontend already renders, carrying the disclosure both as the
//! visible message and as the `degraded_disclosure` prop that
//! `annotate_degraded` established.

use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::ReactiveViewModel;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::view_model::ViewKind;

const DISCLOSURE: &str = "Claude History is not connected — status: Unavailable (binary \
                          'claude-history-mcp' not found on PATH)";

fn string_arg(name: &str, value: &str) -> Arg {
    Arg {
        name: Some(name.to_string()),
        value: RenderExpr::Literal {
            value: holon_api::Value::String(value.to_string()),
        },
    }
}

/// The exact expression the UI watcher emits for a fully-explained failure.
fn disclosed_expr() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "error".to_string(),
        args: vec![
            string_arg("message", DISCLOSURE),
            string_arg("degraded_disclosure", DISCLOSURE),
            string_arg("integration", "claude-history"),
        ],
    }
}

fn interpret(expr: &RenderExpr) -> ReactiveViewModel {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let services = StubBuilderServices::new();
    services.interpret(expr, &RenderContext::default())
}

#[test]
fn the_disclosure_node_is_a_widget_every_frontend_dispatches() {
    let vm = interpret(&disclosed_expr());

    assert_eq!(
        vm.widget_name().as_deref(),
        Some("error"),
        "a name no frontend registers falls to ViewKind::Empty and paints nothing on web"
    );
}

#[test]
fn the_disclosure_survives_into_the_static_view_model_the_web_frontend_dispatches_on() {
    let vm = interpret(&disclosed_expr());
    let static_vm = vm.snapshot();

    let ViewKind::Error { message } = &static_vm.kind else {
        panic!(
            "the disclosure must reach a rendered kind, not ViewKind::Empty; got {:?}",
            static_vm.kind
        );
    };
    assert_eq!(message, DISCLOSURE);
    assert_eq!(
        static_vm.widget_name(),
        Some("error"),
        "dioxus-web dispatches on this; None takes its `empty` arm and renders nothing"
    );
}

/// `ViewKind` is a closed enum, so the extra props do NOT reach the static
/// view model — the disclosure has to be IN the message for a headless oracle
/// to read it. The props remain for renderers that read `ReactiveViewModel`
/// directly (GPUI styles the calmer colour off `degraded_disclosure`).
#[test]
fn the_reactive_node_carries_the_disclosure_props_for_renderers_that_style_them() {
    let vm = interpret(&disclosed_expr());

    assert_eq!(
        vm.prop_str("degraded_disclosure").as_deref(),
        Some(DISCLOSURE)
    );
    assert_eq!(
        vm.prop_str("integration").as_deref(),
        Some("claude-history")
    );
}

/// A plain render failure must stay indistinguishable from before: no
/// disclosure prop, so nothing styles it as a known, named state.
#[test]
fn an_ordinary_error_carries_no_disclosure_prop() {
    let vm = interpret(&RenderExpr::FunctionCall {
        name: "error".to_string(),
        args: vec![string_arg("message", "block 'x' has no query source child")],
    });

    assert_eq!(vm.widget_name().as_deref(), Some("error"));
    assert!(vm.prop_str("degraded_disclosure").is_none());
}

/// What a widget kind nothing registers does TODAY, pinned so nobody reaches
/// for a bespoke kind again.
///
/// Not the mechanism that shipped — while `shadow_builders/degraded.rs`
/// existed the node built fine and died at `to_view_kind` (see the module
/// docs). This is the fate of any name with no builder at all, and it loses
/// the same thing: the disclosure SENTENCE.
#[test]
fn an_unregistered_widget_kind_destroys_the_disclosure_instead_of_showing_it() {
    let vm = interpret(&RenderExpr::FunctionCall {
        name: "degraded".to_string(),
        args: vec![string_arg("message", DISCLOSURE)],
    });

    assert!(
        vm.prop_str("message").is_none(),
        "an unregistered kind keeps none of its props — the disclosure is gone before any \
         frontend gets a chance to paint it"
    );

    let ViewKind::Text { content, .. } = &vm.snapshot().kind else {
        panic!(
            "expected the interpreter's unsupported-widget placeholder; got {:?}",
            vm.snapshot().kind
        );
    };
    assert_eq!(
        content, "[unknown: degraded]",
        "what the user would read instead of why their page is empty"
    );
    assert!(
        !content.contains("Claude History"),
        "and no oracle can recover the integration name from it"
    );
}

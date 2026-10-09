//! A widget builder that panics paints as an error naming the builder, the
//! panic message and its location, between siblings that paint as usual.
//!
//! Run: `cargo test -p holon-gpui --test
//! builder_panic_paints_error_node_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

mod support;

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::render_interpreter::BuilderArgs;
use holon_frontend::shadow_builders::build_shadow_interpreter;
use support::render_reactive_fixture_quiescent_sized;

const MESSAGE: &str = "injected builder panic";

fn panicking(_: BuilderArgs<'_, ReactiveViewModel>) -> ReactiveViewModel {
    panic!("{MESSAGE}")
}

fn call(name: &str, args: Vec<RenderExpr>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args: args
            .into_iter()
            .map(|value| Arg { name: None, value })
            .collect(),
    }
}

fn label(text: &str) -> RenderExpr {
    call(
        "text",
        vec![RenderExpr::Literal {
            value: Value::String(text.to_string()),
        }],
    )
}

#[gpui::test]
fn a_panicking_builder_paints_an_error_between_its_siblings(cx: &mut TestAppContext) {
    let mut interpreter = build_shadow_interpreter();
    interpreter.register("panicking", panicking);
    let services = StubBuilderServices::new().with_interpreter(interpreter);
    let expr = call(
        "column",
        vec![label("before"), call("panicking", vec![]), label("after")],
    );
    let built = std::panic::catch_unwind(AssertUnwindSafe(|| {
        services.interpret(&expr, &RenderContext::default())
    }));
    let tree = built
        .unwrap_or_else(|_| panic!("the builder's panic escaped `interpret`: a window ends there"));

    let snap =
        render_reactive_fixture_quiescent_sized(cx, Arc::new(tree), size(px(900.0), px(600.0)));

    let painted_errors: Vec<String> = snap
        .of_type("error_message")
        .filter_map(|info| info.displayed_text.as_deref().map(str::to_string))
        .collect();
    let [error] = painted_errors.as_slice() else {
        panic!(
            "expected exactly one painted error, got {painted_errors:?}:\n{}",
            snap.dump()
        );
    };
    for part in ["panicking", MESSAGE, file!()] {
        assert!(
            error.contains(part),
            "the painted error must name {part:?}: {error:?}"
        );
    }
    assert_eq!(
        snap.of_type("text").count(),
        2,
        "both siblings of the panicking widget must paint:\n{}",
        snap.dump()
    );
}

mod test_init;

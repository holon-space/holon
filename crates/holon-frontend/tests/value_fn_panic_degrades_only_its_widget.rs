//! A render-DSL value function that panics while a widget's arguments are
//! resolved degrades that widget alone: its siblings and its parent still
//! build, and the run survives the panic.
//!
//! Own test binary: the panic hook is process-global.

use std::panic::AssertUnwindSafe;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::PoisonError;

use holon_api::InterpValue;
use holon_api::Value;
use holon_api::computation::ComputeError;
use holon_api::render_eval::ResolvedArgs;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::panic_record;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::shadow_builders::build_shadow_interpreter;
use holon_frontend::view_model::ViewKind;
use holon_frontend::view_model::ViewModel;

const MESSAGE: &str = "injected value fn panic";

fn boom(
    _: &ResolvedArgs,
    _: &dyn BuilderServices,
    _: &RenderContext,
) -> Result<InterpValue, ComputeError> {
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

fn collect(vm: &ViewModel, errors: &mut Vec<String>, texts: &mut Vec<String>) {
    match &vm.kind {
        ViewKind::Error { message } => errors.push(message.clone()),
        ViewKind::Text { content, .. } => texts.push(content.clone()),
        _ => {}
    }
    for child in vm.children() {
        collect(child, errors, texts);
    }
}

/// The error and text nodes `expr` builds to, or a failure naming `case` when
/// the panic escapes `interpret`.
fn built(
    services: &StubBuilderServices,
    expr: &RenderExpr,
    case: &str,
) -> (Vec<String>, Vec<String>) {
    let tree = std::panic::catch_unwind(AssertUnwindSafe(|| {
        services.interpret(expr, &RenderContext::default())
    }))
    .unwrap_or_else(|_| panic!("{case}: the value fn's panic escaped `interpret`"));
    let (mut errors, mut texts) = (Vec::new(), Vec::new());
    collect(&tree.snapshot(), &mut errors, &mut texts);
    (errors, texts)
}

fn assert_one_error_naming_text_and_the_panic(errors: &[String], case: &str) {
    let [error] = errors else {
        panic!("{case}: expected exactly one error node, got {errors:?}");
    };
    for part in ["text", MESSAGE, file!()] {
        assert!(
            error.contains(part),
            "{case}: the error node must name {part:?}: {error:?}"
        );
    }
}

/// `install` keeps process-wide state, and the tests of this file share a
/// process under `cargo test`.
fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

fn exploding() -> RenderExpr {
    call("text", vec![call("boom", vec![])])
}

/// Build `expr` with `boom` registered, on a fresh bus; the error and text
/// nodes it builds to.
fn build_on_a_fresh_bus(expr: &RenderExpr, case: &str) -> (Vec<String>, Vec<String>) {
    let _serial = serial();
    let config = tempfile::tempdir().expect("a config dir");
    let _bus = panic_record::install(config.path());
    let mut interpreter = build_shadow_interpreter();
    interpreter.register_value_fn("boom", boom);
    let services = StubBuilderServices::new().with_interpreter(interpreter);

    let nodes = built(&services, expr, case);
    assert!(
        !config.path().join(panic_record::RECORD_FILE).exists(),
        "{case}: a caught and shown panic must not be disclosed again as a crash at the next start"
    );
    nodes
}

#[test]
fn a_panicking_value_fn_at_the_root_becomes_an_error_node() {
    let (errors, texts) = build_on_a_fresh_bus(&exploding(), "root");
    assert_one_error_naming_text_and_the_panic(&errors, "root");
    assert!(texts.is_empty(), "root: {texts:?}");
}

#[test]
fn a_panicking_value_fn_leaves_the_parent_and_siblings_of_its_widget_standing() {
    let nested = call(
        "column",
        vec![
            label("before"),
            call("column", vec![exploding(), label("beside")]),
            label("after"),
        ],
    );
    let (errors, texts) = build_on_a_fresh_bus(&nested, "nested");
    assert_eq!(
        texts,
        ["before", "beside", "after"],
        "nested: the parent and the siblings of the widget whose argument panicked must still be built"
    );
    assert_one_error_naming_text_and_the_panic(&errors, "nested");
}

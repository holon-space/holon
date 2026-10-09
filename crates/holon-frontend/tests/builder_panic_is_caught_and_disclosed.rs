//! A widget builder that panics leaves the rest of the tree standing: its node
//! becomes an error naming the builder, the panic message and where it
//! panicked, and the panic shows on the condition bus now. It is not reported
//! again as a crash of this run at the next start.
//!
//! Own test binary: the panic hook is process-global.

use std::panic::AssertUnwindSafe;
use std::time::Duration;
use std::time::Instant;

use holon_api::ConditionBus;
use holon_api::ConditionKind;
use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::panic_record;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::render_interpreter::BuilderArgs;
use holon_frontend::shadow_builders::build_shadow_interpreter;
use holon_frontend::view_model::ViewKind;
use holon_frontend::view_model::ViewModel;

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

fn shown_panic(bus: &ConditionBus) -> Option<(String, String)> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let found = bus.current().iter().find_map(|c| match &c.reason {
            ConditionKind::TaskPanicked { message, .. } if message == MESSAGE => {
                Some((c.subject.clone(), message.clone()))
            }
            _ => None,
        });
        if found.is_some() {
            return found;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    None
}

#[test]
fn a_panicking_builder_becomes_an_error_node_and_a_condition_shown_once() {
    let config = tempfile::tempdir().expect("a config dir");
    let bus = panic_record::install(config.path());

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

    let (mut errors, mut texts) = (Vec::new(), Vec::new());
    collect(&tree.snapshot(), &mut errors, &mut texts);
    assert_eq!(
        texts,
        ["before", "after"],
        "the siblings of the panicking widget must still be built"
    );
    let [error] = errors.as_slice() else {
        panic!("expected exactly one error node, got {errors:?}");
    };
    for part in ["panicking", MESSAGE, file!()] {
        assert!(
            error.contains(part),
            "the error node must name {part:?}: {error:?}"
        );
    }

    let (subject, _) = shown_panic(&bus).expect("the caught panic must show on the condition bus");
    assert!(
        subject.contains(file!()),
        "the condition names where the builder panicked: {subject:?}"
    );
    assert!(
        !config.path().join(panic_record::RECORD_FILE).exists(),
        "a caught and shown panic must not be disclosed again as a crash at the next start"
    );
}

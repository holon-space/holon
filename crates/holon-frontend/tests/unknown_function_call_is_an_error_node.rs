//! A call in value position to a name that is neither a value function nor a
//! widget renders as an error node naming that name — never as a silent
//! `Null`.

use holon_api::render_dsl::parse_render_dsl;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::shadow_builders::build_shadow_interpreter;
use holon_frontend::view_model::ViewKind;
use holon_frontend::view_model::ViewModel;

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

/// The error and text nodes `source` builds to.
fn built(source: &str) -> (Vec<String>, Vec<String>) {
    let expr = parse_render_dsl(source).unwrap_or_else(|e| panic!("{source}: {e:#}"));
    let services = StubBuilderServices::new().with_interpreter(build_shadow_interpreter());
    let tree = services.interpret(&expr, &RenderContext::default());
    let (mut errors, mut texts) = (Vec::new(), Vec::new());
    collect(&tree.snapshot(), &mut errors, &mut texts);
    (errors, texts)
}

fn assert_one_error_naming(source: &str, name: &str) {
    let (errors, texts) = built(source);
    let [error] = errors.as_slice() else {
        panic!("{source}: expected exactly one error node, got errors={errors:?} texts={texts:?}");
    };
    assert!(
        error.contains(&format!("`{name}`")),
        "{source}: the error node must name `{name}`: {error:?}"
    );
    assert!(
        texts.is_empty(),
        "{source}: nothing else may render: {texts:?}"
    );
}

#[test]
fn an_unknown_call_as_an_if_condition_is_an_error_node_not_the_else_branch() {
    assert_one_error_naming(
        r#"if unknown_name(1 + "x") { "THEN" } else { "ELSE" }"#,
        "unknown_name",
    );
}

#[test]
fn an_unknown_call_as_a_widget_argument_is_an_error_node() {
    assert_one_error_naming("text(unknown_fn())", "unknown_fn");
}

#[test]
fn an_unknown_call_inside_an_array_argument_is_an_error_node() {
    assert_one_error_naming(r#"text([unknown_fn(), "b"])"#, "unknown_fn");
}

#[test]
fn widget_calls_as_arguments_still_build() {
    let (errors, texts) = built(r#"column(text("a"), row(text("b")), text("c"))"#);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(texts, ["a", "b", "c"]);
}

#[test]
fn widget_calls_inside_an_array_argument_still_build() {
    let (errors, texts) = built(r#"column([text("a"), text("b")])"#);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(texts, ["a", "b"]);
}

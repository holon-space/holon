//! A call to a registered widget, as the shipped render sources write them, is
//! known as a widget on every path that evaluates render-DSL arguments — never
//! refused as "neither a value function nor a widget".

use std::collections::HashMap;
use std::collections::HashSet;
use std::sync::Arc;

use holon_api::RenderExpr;
use holon_api::Value;
use holon_api::computation::ComputeError;
use holon_api::render_dsl::parse_render_dsl;
use holon_api::render_eval::ResolvedArgs;
use holon_api::render_types::Arg;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::render_interpreter::resolve_props;
use holon_frontend::view_model::ViewKind;
use holon_frontend::view_model::ViewModel;

mod shipped_sources;

/// The argument resolution `selectable`, `input_box`, `question_options` and
/// `resolve_props` share.
fn resolve_shared(
    args: &[Arg],
    ctx: &RenderContext,
    services: &StubBuilderServices,
) -> Result<ResolvedArgs, ComputeError> {
    services.resolve_args(args, ctx)
}

fn operation_params(
    action: &RenderExpr,
    ctx: &RenderContext,
    services: &StubBuilderServices,
) -> Result<(), ComputeError> {
    holon_frontend::operations::parse_action_expr(action, services, ctx).map(drop)
}

/// The evaluation a value outside any render gets (rule action params).
fn plain_value(expr: &RenderExpr) -> Result<Value, ComputeError> {
    holon_api::render_eval::eval_plain_value(expr, &HashMap::<String, Value>::new())
}

fn calls(expr: &RenderExpr) -> Vec<&RenderExpr> {
    let own = matches!(expr, RenderExpr::FunctionCall { .. }).then_some(expr);
    own.into_iter()
        .chain(expr.children().into_iter().flat_map(calls))
        .collect()
}

fn call_name(expr: &RenderExpr) -> &str {
    match expr {
        RenderExpr::FunctionCall { name, .. } => name,
        other => panic!("not a call: {other:?}"),
    }
}

fn call_args(expr: &RenderExpr) -> &[Arg] {
    match expr {
        RenderExpr::FunctionCall { args, .. } => args,
        other => panic!("not a call: {other:?}"),
    }
}

struct Shipped {
    services: StubBuilderServices,
    widgets: HashSet<String>,
    /// `(label, call)` for every call in every shipped render source.
    calls: Vec<(String, RenderExpr)>,
}

fn shipped() -> Shipped {
    let services = StubBuilderServices::new();
    let widgets = holon_frontend::shadow_builders::build_shadow_interpreter().supported_widgets();
    let mut out = Vec::new();
    for (label, source) in shipped_sources::shipped_render_strings() {
        let expr =
            parse_render_dsl(&source).unwrap_or_else(|e| panic!("{label} does not parse: {e:#}"));
        out.extend(calls(&expr).into_iter().map(|c| (label.clone(), c.clone())));
    }
    assert!(
        out.iter().any(|(_, c)| call_name(c) == "selectable")
            && out.iter().any(|(_, c)| call_name(c) == "row"),
        "the shipped corpus lost its selectable(row(..)) sidebar"
    );
    Shipped {
        services,
        widgets,
        calls: out,
    }
}

fn row_ctx() -> RenderContext {
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String("block:b1".to_string()));
    RenderContext::default().with_row(Arc::new(row))
}

/// `Some(widget)` when `err` refuses a call to the registered `widget`.
fn refused_widget<'a>(err: &'a ComputeError, widgets: &HashSet<String>) -> Option<&'a str> {
    match err {
        ComputeError::UnknownFunction { name, .. }
        | ComputeError::NotAPlainValueFunction { name, .. }
            if widgets.contains(name) =>
        {
            Some(name)
        }
        _ => None,
    }
}

fn error_messages(vm: &ViewModel, out: &mut Vec<String>) {
    if let ViewKind::Error { message } = &vm.kind {
        out.push(message.clone());
    }
    for child in vm.children() {
        error_messages(child, out);
    }
}

fn arg(name: &str, value: RenderExpr) -> Arg {
    Arg {
        name: Some(name.to_string()),
        value,
    }
}

fn call(name: &str, args: Vec<Arg>) -> RenderExpr {
    RenderExpr::FunctionCall {
        name: name.to_string(),
        args,
    }
}

fn literal(value: &str) -> RenderExpr {
    RenderExpr::Literal {
        value: Value::String(value.to_string()),
    }
}

/// Every shipped call's own arguments, resolved the way a widget resolves its
/// action template's arguments.
#[test]
fn shipped_call_arguments_resolve_through_the_shared_resolver() {
    let Shipped {
        services,
        widgets,
        calls,
    } = shipped();
    let ctx = row_ctx();
    let refused: Vec<String> = calls
        .iter()
        .filter_map(|(label, c)| {
            let err = resolve_shared(call_args(c), &ctx, &services).err()?;
            let widget = refused_widget(&err, &widgets)?;
            Some(format!(
                "{label}: {}(..) refused widget `{widget}`: {err}",
                call_name(c)
            ))
        })
        .collect();
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}

/// Every shipped widget call as an operation parameter.
#[test]
fn shipped_widget_calls_as_operation_params_are_known() {
    let Shipped {
        services,
        widgets,
        calls,
    } = shipped();
    let ctx = row_ctx();
    let refused: Vec<String> = calls
        .iter()
        .filter(|(_, c)| widgets.contains(call_name(c)))
        .filter_map(|(label, c)| {
            let action = call("block.noop", vec![arg("p", c.clone())]);
            let err = operation_params(&action, &ctx, &services).err()?;
            let widget = refused_widget(&err, &widgets)?;
            Some(format!("{label}: refused widget `{widget}`: {err}"))
        })
        .collect();
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}

/// Every shipped widget call inside the `action:` template of each widget
/// that wires an operation from one.
#[test]
fn shipped_widget_calls_in_action_templates_build_without_refusing_the_widget() {
    let Shipped {
        services,
        widgets,
        calls,
    } = shipped();
    let ctx = row_ctx();
    let mut refused = Vec::new();
    for (label, c) in calls.iter().filter(|(_, c)| widgets.contains(call_name(c))) {
        let action = call("block.noop", vec![arg("p", c.clone())]);
        let hosts = [
            call(
                "selectable",
                vec![
                    Arg {
                        name: None,
                        value: call(
                            "text",
                            vec![Arg {
                                name: None,
                                value: literal("x"),
                            }],
                        ),
                    },
                    arg("action", action.clone()),
                ],
            ),
            call("input_box", vec![arg("action", action.clone())]),
            call(
                "question_options",
                vec![
                    arg("options", literal(r#"[{"label": "yes"}]"#)),
                    arg("action", action.clone()),
                ],
            ),
        ];
        for host in &hosts {
            let mut errors = Vec::new();
            error_messages(&services.interpret(host, &ctx).snapshot(), &mut errors);
            refused.extend(
                errors
                    .into_iter()
                    .filter(|m| m.contains("unknown function") || m.contains("cannot be called in"))
                    .map(|m| format!("{label}: {}: {m}", call_name(host))),
            );
        }
    }
    assert!(refused.is_empty(), "{}", refused.join("\n"));
}

/// The props fast path accepts what the full interpret builds.
#[test]
fn props_fast_path_agrees_with_the_full_interpret_on_shipped_widget_calls() {
    let Shipped {
        services,
        widgets,
        calls,
    } = shipped();
    let ctx = row_ctx();
    let data = Arc::new(ctx.row().clone());
    let mut disagreements = Vec::new();
    for (label, c) in calls.iter().filter(|(_, c)| widgets.contains(call_name(c))) {
        let expr = call(
            "text",
            vec![Arg {
                name: None,
                value: c.clone(),
            }],
        );
        let full = services.interpret(&expr, &ctx).props_update();
        let fast = resolve_props("text", &expr, &data, &services, None);
        if full.is_ok() && fast.is_err() {
            disagreements.push(format!("{label}: {}: {fast:?}", expr.to_rhai()));
        }
    }
    assert!(disagreements.is_empty(), "{}", disagreements.join("\n"));
}

#[test]
fn a_props_fast_path_refusal_names_its_cause() {
    let services = StubBuilderServices::new();
    let ctx = row_ctx();
    let expr = parse_render_dsl("text(no_such_fn())").unwrap();
    let err = resolve_props("text", &expr, &Arc::new(ctx.row().clone()), &services, None)
        .expect_err("an unknown call cannot be a props update");
    assert!(
        format!("{err:?}").contains("no_such_fn"),
        "the refusal must carry the evaluation error naming `no_such_fn`: {err:?}"
    );
}

/// Outside a render no widget is callable; the refusal says so without
/// claiming the name is no widget.
#[test]
fn a_widget_call_in_a_plain_value_is_refused_for_what_it_is() {
    let expr = parse_render_dsl(r#"row(text("a"))"#).unwrap();
    let message = plain_value(&expr)
        .expect_err("a widget has no value outside a render")
        .to_string();
    assert!(message.contains("`row`"), "{message}");
    assert!(
        !message.contains("nor a widget"),
        "a plain-value evaluation does not know the widgets, so it cannot say `row` is none: \
         {message}"
    );
}

/// The shipped left sidebar is the reference user of modifier-click:
/// cmd-click (macOS) / ctrl-click (Windows+Linux) open the page in a tab.
#[test]
fn shipped_left_sidebar_resolves_modifier_click_action_templates() {
    let Shipped {
        services, calls, ..
    } = shipped();
    let (_, sidebar) = calls
        .iter()
        .find(|(label, c)| {
            label == "default/index.org#0"
                && call_name(c) == "selectable"
                && call_args(c)
                    .iter()
                    .any(|a| a.name.as_deref() == Some("cmd_action"))
        })
        .expect("the left sidebar's modifier-click selectable in assets/default/index.org");
    let resolved = resolve_shared(call_args(sidebar), &row_ctx(), &services).unwrap();

    // `action` is allowlisted, so its presence proves the fixture reached
    // `selectable` — a failure below is then about the modifier-click names.
    assert!(
        resolved.get_template("action").is_some(),
        "primary `action` template missing — the fixture is malformed"
    );
    for key in ["cmd_action", "ctrl_action"] {
        assert!(
            resolved.get_template(key).is_some(),
            "`{key}` did not resolve as a template, so the modifier-click action is dead"
        );
    }
}

//! Contract: a render-spec `live_query` node carries a `query_context_id` that
//! the vault's own query rows supply. A value that forms no URI must paint the
//! error banner naming it, never unwind inside the URI constructor — the
//! unwinding render kills the whole window rather than one region.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! live_query_context_id_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

mod support;

use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::render_types::Arg;
use holon_api::render_types::RenderExpr;
use holon_frontend::reactive_view_model::ReactiveSlot;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use support::render_reactive_fixture_quiescent_sized;

/// `list(#{item_template: text(col("content"))})` — a template the builder's
/// own render-expr validation passes, so the context id is what fails.
fn item_template() -> RenderExpr {
    RenderExpr::FunctionCall {
        name: "list".to_string(),
        args: vec![Arg {
            name: Some("item_template".to_string()),
            value: RenderExpr::FunctionCall {
                name: "text".to_string(),
                args: vec![Arg {
                    name: None,
                    value: RenderExpr::FunctionCall {
                        name: "col".to_string(),
                        args: vec![Arg {
                            name: None,
                            value: RenderExpr::Literal {
                                value: Value::String("content".to_string()),
                            },
                        }],
                    },
                }],
            },
        }],
    }
}

/// The node `shadow_builders::live_query` emits for a query source, with the
/// context id a row supplied.
fn live_query_node(context_id: &str) -> Arc<ReactiveViewModel> {
    let props = [
        ("query", "SELECT 1".to_string()),
        ("query_lang", "holon_sql".to_string()),
        ("query_context_id", context_id.to_string()),
        (
            "render_expr",
            serde_json::to_string(&item_template()).expect("the template serializes"),
        ),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), Value::String(v)))
    .collect();

    let mut node = ReactiveViewModel::from_widget("live_query", props);
    node.slot = Some(ReactiveSlot::new(ReactiveViewModel::from_widget(
        "column",
        Default::default(),
    )));
    Arc::new(node)
}

#[gpui::test]
fn a_context_id_that_forms_no_uri_paints_the_error_banner(cx: &mut TestAppContext) {
    let snap = render_reactive_fixture_quiescent_sized(
        cx,
        live_query_node("my task"),
        size(px(900.0), px(600.0)),
    );

    assert!(
        !snap.entries.is_empty(),
        "the window tracked NO elements at all — the fixture painted nothing, so the \
         assertion below would be vacuous"
    );
    let painted: Vec<String> = snap
        .of_type("error")
        .filter_map(|info| info.displayed_text.as_ref().map(|t| t.to_string()))
        .collect();
    assert!(
        painted.iter().any(|t| t.contains("my task")),
        "no painted error names the value the vault wrote. Tracked: {painted:?}",
    );
}

mod test_init;

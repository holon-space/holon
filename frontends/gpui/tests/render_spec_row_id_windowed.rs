//! Contract: the row a `rendered_text` / `editable_text` node is bound to comes
//! from the `id` column of its data row — a value the vault's own `live_query`
//! chooses. A value that forms no URI must paint the error banner naming it,
//! never unwind inside the URI constructor, which kills the whole window.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! render_spec_row_id_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

mod support;

use std::sync::Arc;

use futures_signals::signal::Mutable;
use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use support::BoundsSnapshot;
use support::render_reactive_fixture_quiescent_sized;

/// The node the real shadow builder emits for `dsl`, bound to a row whose `id`
/// column is what the vault's SQL chose.
fn node_for_row(dsl: &str, id: &str) -> Arc<ReactiveViewModel> {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let mut row = DataRow::new();
    row.insert("id".to_string(), Value::String(id.to_string()));
    row.insert("content".to_string(), Value::String("hello".to_string()));
    let ctx = RenderContext::default().with_row_mutable(Mutable::new(Arc::new(row)).read_only());
    let expr = holon_api::render_dsl::parse_render_dsl(dsl).expect("dsl should parse");
    Arc::new(StubBuilderServices::new().interpret(&expr, &ctx))
}

fn painted_error(cx: &mut TestAppContext, dsl: &str, id: &str) -> BoundsSnapshot {
    let snap = render_reactive_fixture_quiescent_sized(
        cx,
        node_for_row(dsl, id),
        size(px(900.0), px(600.0)),
    );
    assert!(
        !snap.entries.is_empty(),
        "the window tracked NO elements at all — the fixture painted nothing, so the \
         assertion below would be vacuous"
    );
    snap
}

fn names_the_value(snap: &BoundsSnapshot, expected: &str) -> bool {
    snap.of_type("error")
        .filter_map(|info| info.displayed_text.as_ref())
        .any(|text| text.contains(expected))
}

#[gpui::test]
fn a_rendered_text_row_id_that_forms_no_uri_paints_the_error_banner(cx: &mut TestAppContext) {
    let snap = painted_error(cx, "rendered_text(col(\"content\"))", "my task");

    assert!(
        names_the_value(&snap, "my task"),
        "no painted error names the row id the vault wrote. Tracked: {:?}",
        snap.of_type("error")
            .map(|i| i.displayed_text.clone())
            .collect::<Vec<_>>()
    );
}

#[gpui::test]
fn an_editable_text_row_id_that_forms_no_uri_paints_the_error_banner(cx: &mut TestAppContext) {
    let snap = painted_error(cx, "editable_text(col(\"content\"))", "my task");

    assert!(
        names_the_value(&snap, "my task"),
        "no painted error names the row id the vault wrote. Tracked: {:?}",
        snap.of_type("error")
            .map(|i| i.displayed_text.clone())
            .collect::<Vec<_>>()
    );
}

mod test_init;

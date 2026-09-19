//! Contract: a `live_block`'s block id is parsed once, and a value that forms
//! no URI paints an error banner naming it. Neither leg may re-scheme the
//! failed value through `EntityUri::block`, which unwinds inside the URI
//! constructor on the very same input and kills the whole window.
//!
//! Two legs carry such a value, and they paint through different surfaces:
//! - the author-written render DSL (`live_block("my task")`), refused by the
//!   shadow builder into an `error` node, painted by `builders::error::render`
//!   whose text rides an `error_message` tracker; and
//! - the `block_id` prop the gpui builder reads back off the node, refused by
//!   the builder's own `error_banner`, which tracks as `error`.
//!
//! Run: `cargo test -p holon-gpui --features pbt --test
//! live_block_id_windowed -- --test-threads=1`
//! ⚠ `--test-threads=1` mandatory (gpui `HeadlessAppContext` is not
//! parallel-safe).

mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_frontend::RenderContext;
use holon_frontend::StubBuilderServices;
use holon_frontend::reactive::BuilderServices;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use support::BoundsSnapshot;
use support::render_reactive_fixture_quiescent_sized;

const UNPARSEABLE: &str = "my task";

fn painted(cx: &mut TestAppContext, node: Arc<ReactiveViewModel>) -> BoundsSnapshot {
    let snap = render_reactive_fixture_quiescent_sized(cx, node, size(px(900.0), px(600.0)));
    assert!(
        !snap.entries.is_empty(),
        "the window tracked NO elements at all — the fixture painted nothing, so the \
         assertion below would be vacuous"
    );
    snap
}

/// Every message the frame painted for a refusal, from either surface: the
/// `error` widget's own `error_message` tracker and the builders'
/// `error_banner`.
fn error_texts(snap: &BoundsSnapshot) -> Vec<String> {
    ["error_message", "error"]
        .iter()
        .flat_map(|t| snap.of_type(t))
        .filter_map(|info| info.displayed_text.as_ref().map(|t| t.to_string()))
        .collect()
}

#[gpui::test]
fn a_dsl_live_block_id_that_forms_no_uri_paints_the_error_banner(cx: &mut TestAppContext) {
    holon_frontend::shadow_builders::register_render_dsl_widget_names();
    let expr = holon_api::render_dsl::parse_render_dsl(&format!("live_block(\"{UNPARSEABLE}\")"))
        .expect("dsl should parse");
    let node = Arc::new(StubBuilderServices::new().interpret(&expr, &RenderContext::default()));

    let snap = painted(cx, node);
    let texts = error_texts(&snap);

    assert!(
        texts.iter().any(|t| t.contains(UNPARSEABLE)),
        "no painted error names the id the render expression wrote. Painted: {texts:?}"
    );
}

#[gpui::test]
fn a_live_block_prop_that_forms_no_uri_paints_the_error_banner(cx: &mut TestAppContext) {
    let mut props: HashMap<String, Value> = HashMap::new();
    props.insert(
        "block_id".to_string(),
        Value::String(UNPARSEABLE.to_string()),
    );
    let node = Arc::new(ReactiveViewModel::from_widget("live_block", props));

    let snap = painted(cx, node);
    let texts = error_texts(&snap);

    assert!(
        texts.iter().any(|t| t.contains(UNPARSEABLE)),
        "no painted error names the block_id the node carried. Painted: {texts:?}"
    );
}

mod test_init;

//! The MCP `scroll` tool must scroll an EAGER `overflow_y_scroll` panel.
//!
//! Dogfood 2026-07-23: the MCP `scroll` tool routed every request through the
//! `ScrollList` interaction event -> `ListState::scroll_by`. Eager
//! content-height panels (the seeded main panel: `column(collection...)`, whose
//! `column.rs` render is a plain `overflow_y_scroll` `div`, NOT a `gpui::list`)
//! have no `ListState`, so the ListState-only path no-oped - and worse, older
//! revisions reported success while nothing moved (the oracle trap).
//!
//! The fix reroutes the tool to dispatch a `MouseMove` + `ScrollWheel` pair
//! (the same events the driver now emits into the interaction pump). The
//! MouseMove satisfies gpui's `Hitbox::should_handle_scroll` hover gate - a
//! bare off-cursor `ScrollWheel` is silently dropped by that gate, which is the
//! exact reason the naive synthetic-wheel path no-oped.
//!
//! This test pins the fixed emission against the production eager-panel fixture
//! (shared with `main_panel_scroll.rs`): the `MouseMove` + `ScrollWheel` pair
//! the fixed tool emits reveals a below-fold row. It also asserts the MCP-layer
//! conversion (`interaction_event_to_platform_inputs`) turns the tool's two
//! `InteractionEvent`s into exactly that `MouseMove` + `ScrollWheel` pair, so
//! the windowed proof is wired to the real pump path, not a hand-rolled one.
//!
//! Run: `cargo test -p holon-gpui --test mcp_scroll_wheel_eager_panel`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::point;
use gpui::px;
use gpui::size;
use holon_api::Value;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::entity_view_registry::EntityCache;
use holon_gpui::geometry::BoundsRegistry;
use holon_mcp::server::InteractionEvent;
use support::ReactiveFixtureView;
use support::simulate_wheel_at;

const VIEWPORT_W: f32 = 600.0;
const VIEWPORT_H: f32 = 400.0;
const ITEM_COUNT: usize = 80;
const FAR_IX: usize = 60;

fn item_id(ix: usize) -> String {
    format!("test-item-{ix}")
}

/// A `text` row whose production render path records bounds keyed by `data.id`.
fn text_item(ix: usize) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".into(), Value::String(item_id(ix)));
    data.insert("content".into(), Value::String(format!("row {ix}")));

    let mut props = HashMap::new();
    props.insert("content".into(), Value::String(format!("row {ix}")));
    props.insert("field".into(), Value::String("content".into()));

    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(Arc::new(data)).read_only();
    vm
}

/// `columns( column( <list-collection> ) )` - the production shape of an eager
/// content-height main panel (see `main_panel_scroll.rs` for why the `column`
/// wrapper forces the eager `overflow_y_scroll` render with no `ListState`).
fn main_panel_columns_root() -> Arc<ReactiveViewModel> {
    let items: Vec<ReactiveViewModel> = (0..ITEM_COUNT).map(text_item).collect();
    let inner_view = Arc::new(ReactiveView::new_static_with_layout(
        items,
        CollectionVariant::list(0.0),
    ));
    let collection_child = Arc::new(ReactiveViewModel {
        collection: Some(inner_view),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    });

    let mut main_column = ReactiveViewModel::from_widget("column", HashMap::new());
    main_column.children = vec![collection_child];

    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![main_column],
        CollectionVariant::columns(4.0),
    ));
    Arc::new(ReactiveViewModel {
        collection: Some(columns_view),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    })
}

fn visible_height(registry: &BoundsRegistry, entity_id: &str) -> Option<f32> {
    registry
        .all_elements()
        .iter()
        .find(|(_, info)| info.entity_id.as_deref() == Some(entity_id))
        .map(|(_, info)| info.height)
}

/// The tool's fixed emission - a MouseMove then a ScrollWheel at the same point
/// - must scroll the eager panel and reveal a below-fold row. Reproduces the
/// `MouseMove` + `ScrollWheel` sequence `GpuiUserDriver::scroll_at` now sends.
#[gpui::test]
fn mcp_scroll_reveals_below_fold_row_on_eager_panel(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let root = main_panel_columns_root();
    let bounds = BoundsRegistry::new();
    let (_entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| {
            ReactiveFixtureView::with_bounds(root, size(px(VIEWPORT_W), px(VIEWPORT_H)), bounds)
        }
    });
    vcx.run_until_parked();
    bounds.flush();

    let far_before = visible_height(&bounds, &item_id(FAR_IX)).unwrap_or(0.0);
    assert!(
        far_before <= 0.0,
        "row {FAR_IX} should start below the fold (got height {far_before})"
    );

    // The CURRENT MCP primitive (`scroll_list_by`, reached by the `scroll`
    // tool's `ScrollList` event) is ListState-only. The eager panel exposes no
    // `ListState`, so it reports not-scrolled and the below-fold row stays
    // clipped. This is the no-op the reroute replaces.
    let cache = EntityCache::default();
    let scrolled = vcx.update(|_, cx| {
        holon_gpui::scroll_list_by("block:default-main-panel", -1600.0, &cache, cx)
    });
    assert_eq!(
        scrolled,
        Ok(false),
        "the ListState-only MCP primitive must report not-scrolled for the eager panel"
    );
    vcx.run_until_parked();
    bounds.flush();
    assert!(
        visible_height(&bounds, &item_id(FAR_IX)).unwrap_or(0.0) <= 0.0,
        "the ListState-only path must not scroll the eager panel"
    );

    // The FIXED emission: a MouseMove then a ScrollWheel at the panel centre
    // (what `GpuiUserDriver::scroll_at` now sends into the interaction pump).
    // `simulate_wheel_at` dispatches exactly that pair.
    simulate_wheel_at(
        vcx,
        point(px(VIEWPORT_W / 2.0), px(VIEWPORT_H / 2.0)),
        px(-1600.0),
    );
    vcx.run_until_parked();
    bounds.flush();

    let far_after = visible_height(&bounds, &item_id(FAR_IX)).unwrap_or(0.0);
    assert!(
        far_after > 0.0,
        "after the MouseMove + ScrollWheel the MCP tool now emits, row {FAR_IX} should be revealed \
         (nonzero visible height) but was still clipped (height {far_after})."
    );

    // Wire the windowed proof to the real pump conversion: the tool's two
    // InteractionEvents must lower to exactly one MouseMove then one
    // ScrollWheel PlatformInput.
    let center = (VIEWPORT_W / 2.0, VIEWPORT_H / 2.0);
    let mv = holon_gpui::interaction_event_to_platform_inputs(&InteractionEvent::MouseMove {
        position: center,
        pressed_button: None,
        modifiers: Vec::new(),
    });
    let wheel = holon_gpui::interaction_event_to_platform_inputs(&InteractionEvent::ScrollWheel {
        position: center,
        delta: (0.0, -40.0),
        modifiers: Vec::new(),
    });
    assert!(
        matches!(mv.as_slice(), [gpui::PlatformInput::MouseMove(_)]),
        "MCP MouseMove must lower to a single MouseMove PlatformInput, got {mv:?}"
    );
    assert!(
        matches!(wheel.as_slice(), [gpui::PlatformInput::ScrollWheel(_)]),
        "MCP ScrollWheel must lower to a single ScrollWheel PlatformInput, got {wheel:?}"
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;

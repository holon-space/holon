//! The MCP `scroll` tool must scroll a ScrollHandle-driven region — the
//! bounded backlinks accordion whose body is its OWN `min_h_0 +
//! overflow_y_scroll` viewport (NOT a `ListState`-backed `gpui::list`).
//!
//! This is the accordion counterpart to `mcp_scroll_wheel_eager_panel.rs`.
//! PR #85 (`fix(mcp): scroll tool drives real synthetic wheel events`, commit
//! e9d6e75e — landed) rerouted `scroll_at`/`scroll_entity` through
//! `dispatch_wheel_and_settle` (synthetic `MouseMove` + `ScrollWheel` via the
//! interaction pump, geometry-fingerprint fail-loud) and DEMOTED the
//! `ListState::scroll_by` path to a fallback. Its commit message closes the
//! vault TODO "Route the MCP scroll tool through the panel ScrollHandle, not
//! only ListState". That fix was proven against the EAGER main panel; the
//! nested, capped accordion body (a distinct region shape — its own overflow
//! viewport inside a `max_h(relative(f))` cap) was only ever proven with a raw
//! `simulate_wheel_at` (`accordion_bounded_pbt::wheel_over_accordion_body_*`),
//! never against the MCP tool's own emission + fail-loud primitive.
//!
//! This test pins that missing combination for the ScrollHandle region:
//!   1. the `ListState`-only primitive (`scroll_list_by`, reached by the tool's
//!      `ScrollList` event) reports not-scrolled (`Ok(false)`) — never a fake
//!      success — for the accordion, because the accordion body is not a
//!      `ReactiveShell` `ListState`;
//!   2. the tool's fixed emission (a `MouseMove` then a `ScrollWheel`, the exact
//!      pair `GpuiUserDriver::scroll_at` dispatches) scrolls the capped
//!      accordion body and reveals a below-fold backlink row;
//!   3. the MCP-layer conversion (`interaction_event_to_platform_inputs`) lowers
//!      those two `InteractionEvent`s to exactly that `MouseMove` + `ScrollWheel`
//!      `PlatformInput` pair, wiring the windowed proof to the real pump path.
//!
//! NOTE: because the capability already landed (PR #85), this is a GREEN
//! regression lock, not a red-first feature test. It closes a coverage gap
//! (no test drove the MCP tool primitive against a ScrollHandle region), it
//! does not introduce new behavior.
//!
//! Run: `cargo test -p holon-gpui --test mcp_scroll_wheel_accordion`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use futures_signals::signal::Mutable;
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
const BACKLINK_COUNT: usize = 80;

fn backlink_id(ix: usize) -> String {
    format!("backlink-{ix}")
}

/// Production-faithful `text` row (see `builders/text.rs`): `data.id` +
/// `props.field == "content"` makes its bounds appear keyed by `entity_id`.
fn text_item(id: String, label: String) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".into(), Value::String(id));
    data.insert("content".into(), Value::String(label.clone()));

    let mut props = HashMap::new();
    props.insert("content".into(), Value::String(label));
    props.insert("field".into(), Value::String("content".into()));

    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = Mutable::new(Arc::new(data)).read_only();
    vm
}

/// `columns( column( accordion(<backlinks>) ) )` — the seed's Linked-references
/// shape, with `fraction=1.0` and no outline so the accordion body owns the
/// whole panel (structurally like the eager `main_panel_scroll` fixture, but
/// the body is a `max_h`-capped `overflow_y_scroll` region, not the panel-wide
/// eager scroller). The accordion is a legitimately-placed direct column child
/// so `columns::render` routes it through the flow-panel split.
fn accordion_root() -> Arc<ReactiveViewModel> {
    let backlink_rows: Vec<Arc<ReactiveViewModel>> = (0..BACKLINK_COUNT)
        .map(|i| Arc::new(text_item(backlink_id(i), format!("backlink {i}"))))
        .collect();
    let mut acc_props = HashMap::new();
    acc_props.insert(
        "title".to_string(),
        Value::String("Linked references".into()),
    );
    acc_props.insert("max_height_fraction".to_string(), Value::Float(1.0));
    acc_props.insert("collapsible".to_string(), Value::Boolean(true));
    acc_props.insert("collapsed".to_string(), Value::Boolean(false));
    let accordion = Arc::new(ReactiveViewModel {
        children: backlink_rows,
        expanded: Some(Mutable::new(true)),
        ..ReactiveViewModel::from_widget("accordion", acc_props)
    });

    let mut column = ReactiveViewModel::from_widget("column", HashMap::new());
    column.children = vec![accordion];

    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![column],
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

#[gpui::test]
fn mcp_scroll_reveals_below_fold_backlink_in_accordion(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_component::init(cx);
    });

    let far = BACKLINK_COUNT - 1;
    let root = accordion_root();
    let bounds = BoundsRegistry::new();
    let (_entity, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        move |_, _| {
            ReactiveFixtureView::with_bounds(root, size(px(VIEWPORT_W), px(VIEWPORT_H)), bounds)
        }
    });
    vcx.run_until_parked();
    bounds.flush();

    let far_before = visible_height(&bounds, &backlink_id(far)).unwrap_or(0.0);
    assert!(
        far_before <= 0.0,
        "backlink {far} should start below the accordion body fold (got {far_before})"
    );

    // The `ListState`-only MCP primitive (`scroll_list_by`, reached by the
    // `scroll` tool's `ScrollList` event) cannot reach the accordion body: it is
    // a `min_h_0 + overflow_y_scroll` viewport, not a `ReactiveShell` `ListState`.
    // It must report not-scrolled — never fake success (the oracle trap PR #85
    // killed). An empty cache is the same fail-loud signal the eager-panel test
    // pins: no panel shell => no ListState reachable.
    let cache = EntityCache::default();
    let scrolled = vcx
        .update(|_, cx| holon_gpui::scroll_list_by("block:default-main-panel", -1600.0, &cache, cx));
    assert_eq!(
        scrolled,
        Ok(false),
        "the ListState-only MCP primitive must report not-scrolled for the \
         ScrollHandle-driven accordion body, never a fake success"
    );
    vcx.run_until_parked();
    bounds.flush();
    assert!(
        visible_height(&bounds, &backlink_id(far)).unwrap_or(0.0) <= 0.0,
        "the ListState-only path must not scroll the accordion body"
    );

    // The tool's fixed emission: a MouseMove then a ScrollWheel at the accordion
    // body centre (what `GpuiUserDriver::scroll_at` dispatches into the
    // interaction pump). `simulate_wheel_at` sends exactly that pair. This drives
    // the ScrollHandle-backed `overflow_y_scroll` viewport.
    simulate_wheel_at(vcx, point(px(VIEWPORT_W / 2.0), px(220.0)), px(-100000.0));
    vcx.run_until_parked();
    bounds.flush();

    let far_after = visible_height(&bounds, &backlink_id(far)).unwrap_or(0.0);
    assert!(
        far_after > 0.0,
        "after the MouseMove + ScrollWheel the MCP tool emits, backlink {far} \
         should be revealed (nonzero visible height) in the ScrollHandle-driven \
         accordion body but was still clipped (height {far_after})."
    );

    // Wire the windowed proof to the real pump conversion: the tool's two
    // InteractionEvents must lower to exactly one MouseMove then one ScrollWheel.
    let center = (VIEWPORT_W / 2.0, 220.0);
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

//! The LEFT SIDEBAR must scroll (Martin dogfood 2026-07-30, real vault: the
//! page tree grew past the viewport and the wheel did nothing).
//!
//! The sidebar is a SHRINK DRAWER column, not a flow panel: `columns::render`'s
//! drawer branch builds its own `h_full` wrapper chain instead of
//! `panel_wrap`'s `flex_1 relative -> absolute size_full`. Every existing
//! scroll rung (`plain_path_scroll`, `main_panel_scroll`,
//! `mcp_scroll_wheel_eager_panel`) wheels at the WINDOW CENTRE — i.e. over the
//! main panel — and the one sidebar-flavoured rung
//! (`plain_path_scroll::shell_wrapped_sidebar_scrolls`) mounts the sidebar
//! block as a FLOW child. So the drawer wrapper chain the real sidebar hangs
//! from was never under a wheel.
//!
//! This rung mounts the production shape — `columns(drawer(shrink,
//! live_block(block:default-left-sidebar)), <main column>)` — and wheels over
//! the SIDEBAR's x band.
//!
//! Run: `cargo test -p holon-gpui --test left_sidebar_scroll`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::TestAppContext;
use gpui::point;
use gpui::px;
use gpui::size;
use holon_api::EntityUri;
use holon_api::Value;
use holon_frontend::LayoutHint;
use holon_frontend::geometry::GeometryProvider;
use holon_frontend::reactive_view::ReactiveView;
use holon_frontend::reactive_view_model::CollectionVariant;
use holon_frontend::reactive_view_model::ReactiveSlot;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_gpui::geometry::BoundsRegistry;
use support::BlockTreeRegistry;
use support::BlockTreeThunk;
use support::ReactiveFixtureView;
use support::simulate_wheel_at;

const ITEM_COUNT: usize = 56;
const FAR_IX: usize = 50;
const VIEWPORT_W: f32 = 700.0;
const VIEWPORT_H: f32 = 500.0;
const SIDEBAR_W: f32 = 200.0;
const SIDEBAR_BLOCK: &str = "block:default-left-sidebar";
const SIDEBAR_LOCAL: &str = "default-left-sidebar";

fn page_row(ix: usize) -> ReactiveViewModel {
    let mut data = HashMap::new();
    data.insert("id".to_string(), Value::String(format!("page-{ix}")));
    data.insert("content".to_string(), Value::String(format!("Page {ix}")));
    let mut props = HashMap::new();
    props.insert("content".to_string(), Value::String(format!("Page {ix}")));
    props.insert("field".to_string(), Value::String("content".to_string()));
    let mut vm = ReactiveViewModel::from_widget("text", props);
    vm.data = futures_signals::signal::Mutable::new(Arc::new(data)).read_only();
    vm
}

/// The sidebar's block tree AS THE LIVE APP RENDERS IT:
/// `view_mode_switcher(slot: column(<page collection>, divider()))`.
///
/// The seeded `left_sidebar::render::0` source is a bare `column(tree(...),
/// divider(), …)`, but a block with ≥2 render variants is wrapped by
/// `view_mode_switcher_from_variants` — which Martin's sidebar block is
/// (`describe_ui block:default-left-sidebar` on the live app returns
/// `view_mode_switcher > column > tree [96 items]`). The `modes` prop must
/// hold ≥1 entry or `build_switcher_bar` returns `None` and the switcher
/// renders its slot straight through.
fn register_sidebar(registry: &BlockTreeRegistry, with_switcher: bool) {
    let thunk: BlockTreeThunk = Arc::new(move || {
        let items: Vec<ReactiveViewModel> = (0..ITEM_COUNT).map(page_row).collect();
        let view = Arc::new(ReactiveView::new_static_with_layout(
            items,
            CollectionVariant::list(0.0),
        ));
        let collection = ReactiveViewModel {
            collection: Some(view),
            ..ReactiveViewModel::from_widget("list", HashMap::new())
        };
        let mut col = ReactiveViewModel::from_widget("column", HashMap::new());
        col.children = vec![
            Arc::new(collection),
            Arc::new(ReactiveViewModel::from_widget("divider", HashMap::new())),
        ];

        if !with_switcher {
            return col;
        }

        let mut props = HashMap::new();
        props.insert(
            "entity_uri".to_string(),
            Value::String(SIDEBAR_BLOCK.to_string()),
        );
        props.insert(
            "modes".to_string(),
            Value::String(
                "[{\"name\":\"tree\",\"icon\":\"list\"},{\"name\":\"table\",\"icon\":\"table\"}]"
                    .to_string(),
            ),
        );
        props.insert("active_mode".to_string(), Value::String("tree".to_string()));
        ReactiveViewModel {
            slot: Some(ReactiveSlot::new(col)),
            ..ReactiveViewModel::from_widget("view_mode_switcher", props)
        }
    });
    registry.register(SIDEBAR_BLOCK, vec![("default".to_string(), thunk)], 0);
}

/// `drawer(#{mode: "shrink"}, live_block(block:default-left-sidebar))` — the
/// production left-sidebar node.
fn sidebar_drawer() -> ReactiveViewModel {
    let mut props = HashMap::new();
    props.insert("mode".to_string(), Value::String("shrink".to_string()));
    props.insert(
        "block_id".to_string(),
        Value::String(SIDEBAR_BLOCK.to_string()),
    );
    props.insert("width".to_string(), Value::Float(SIDEBAR_W as f64));
    ReactiveViewModel {
        children: vec![Arc::new(ReactiveViewModel::live_block(EntityUri::block(
            SIDEBAR_LOCAL,
        )))],
        layout_hint: LayoutHint::Fixed { px: SIDEBAR_W },
        ..ReactiveViewModel::from_widget("drawer", props)
    }
}

/// `columns(drawer(left sidebar), column(text))` — sidebar + a trivial flow
/// panel, so `columns::render` takes its shrink-drawer branch exactly as
/// production does.
fn root() -> Arc<ReactiveViewModel> {
    let mut main = ReactiveViewModel::from_widget("column", HashMap::new());
    main.children = vec![Arc::new(ReactiveViewModel::text("main panel"))];
    let columns_view = Arc::new(ReactiveView::new_static_with_layout(
        vec![sidebar_drawer(), main],
        CollectionVariant::columns(4.0),
    ));
    Arc::new(ReactiveViewModel {
        collection: Some(columns_view),
        ..ReactiveViewModel::from_widget("columns", HashMap::new())
    })
}

fn visible_height(bounds: &BoundsRegistry, entity_id: &str) -> Option<f32> {
    bounds
        .all_elements()
        .iter()
        .find(|(_, i)| i.entity_id.as_deref() == Some(entity_id))
        .map(|(_, i)| i.height)
}

/// Wheel over the SIDEBAR band (x inside the drawer's width), not the window
/// centre. Returns the far row's visible height before and after.
fn run_sidebar_wheel(cx: &mut TestAppContext, with_switcher: bool) -> (f32, f32) {
    let registry = Arc::new(BlockTreeRegistry::new());
    register_sidebar(&registry, with_switcher);
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> =
        support::TestServices::with_registry_quiescent(registry);
    let bounds = BoundsRegistry::new();
    let (_e, vcx) = cx.add_window_view({
        let bounds = bounds.clone();
        let services = services.clone();
        move |_, _| {
            ReactiveFixtureView::with_services_and_bounds(
                root(),
                services,
                size(px(VIEWPORT_W), px(VIEWPORT_H)),
                bounds,
            )
        }
    });
    vcx.run_until_parked();
    bounds.flush();
    let before = visible_height(&bounds, &format!("page-{FAR_IX}")).unwrap_or(0.0);
    simulate_wheel_at(
        vcx,
        point(px(SIDEBAR_W / 2.0), px(VIEWPORT_H / 2.0)),
        px(-4000.0),
    );
    vcx.run_until_parked();
    bounds.flush();
    let after = visible_height(&bounds, &format!("page-{FAR_IX}")).unwrap_or(0.0);
    (before, after)
}

/// Martin's live shape: the sidebar block has ≥2 render variants, so its tree
/// root is a `view_mode_switcher`.
#[gpui::test]
fn left_sidebar_with_view_mode_switcher_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let (before, after) = run_sidebar_wheel(cx, true);
    assert!(
        before <= 0.0,
        "page-{FAR_IX} should start below the fold (got visible height {before})"
    );
    assert!(
        after > 0.0,
        "LEFT SIDEBAR DOES NOT SCROLL: after a wheel over the sidebar band, page \
         row {FAR_IX} should have scrolled into view but was still clipped \
         (visible height {after}). The root `view_mode_switcher` renders its \
         slot content ABSOLUTELY inside a `size_full` outer div, so the shell's \
         `overflow_y_scroll` viewport sees a child exactly its own height — \
         scroll max 0, wheel no-op."
    );
}

/// Control: a single-variant sidebar (no `view_mode_switcher` wrapper) already
/// scrolls. Attributes the failure above to the switcher, and guards the fix
/// against regressing the plain shape.
#[gpui::test]
fn left_sidebar_without_view_mode_switcher_scrolls(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));
    let (before, after) = run_sidebar_wheel(cx, false);
    assert!(
        before <= 0.0,
        "page-{FAR_IX} should start below the fold (got visible height {before})"
    );
    assert!(
        after > 0.0,
        "plain (no switcher) sidebar must scroll: page-{FAR_IX} still clipped ({after})"
    );
}

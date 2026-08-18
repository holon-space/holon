//! The LEFT SIDEBAR must scroll all the way to its LAST row (Martin dogfood
//! 2026-08-18, real vault: the page tree ends at `Templates > Compass` and the
//! Integrations section below it is drawn cut off at the window bottom).
//!
//! `left_sidebar_scroll` proves the sidebar scrolls AT ALL. This rung asks the
//! next question: after scrolling to the scroll MAXIMUM, is the last authored
//! row inside the window? The seeded sidebar column stacks a scrollable
//! collection (`tree`) with three trailing content-height siblings (`divider`,
//! the Integrations header `row`, the `live_query`), so a scroll extent
//! computed from the collection alone would strand exactly those siblings
//! below the fold.
//!
//! This rung does NOT currently reproduce Martin's report — it is green at the
//! production sidebar shape (bugfunnel
//! `2026-08-18-left-sidebar-tail-unreachable-at-scroll-max`, which records what
//! it rules out). It stands as the regression guard for the property while the
//! live-app measurement that would pin the real mechanism is pending.
//!
//! It also mounts the content wrapper under window chrome
//! ([`ReactiveFixtureView::with_page_chrome`]) — production stacks it under a
//! title bar inside `HolonApp::render`'s `page` (`lib.rs`), where every other
//! windowed fixture mounts it as the whole window. That parity gap is closed
//! here whether or not it is this bug's cause.
//!
//! Run: `cargo test -p holon-gpui --test sidebar_scroll_reaches_bottom`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::Arc;

use gpui::AppContext;
use gpui::Bounds;
use gpui::Point;
use gpui::TestAppContext;
use gpui::VisualTestContext;
use gpui::WindowBounds;
use gpui::WindowHandle;
use gpui::point;
use gpui::px;
use gpui::size;
use holon_api::EntityUri;
use holon_api::Value;
use holon_api::render_dsl::parse_render_dsl;
use holon_frontend::LayoutHint;
use holon_frontend::RenderContext;
use holon_frontend::geometry::ElementInfo;
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

/// The bundled seed, at compile time — the SAME asset `holon_app::seed` embeds.
const SEED_ORG: &str = include_str!("../../../assets/default/index.org");

/// Every constant below is MEASURED from Martin's running app over the live
/// MCP (`describe_ui` reports real per-element geometry): window 1512x948
/// logical, 115 tree items, sidebar content starting at y=104.0, and exactly
/// ONE Integrations row.
const PAGE_COUNT: usize = 115;
const WINDOW_W: f32 = 1512.0;
const WINDOW_H: f32 = 948.0;
/// The three chrome bars production stacks above the content wrapper
/// (`lib.rs` `HolonApp::render`'s `page`): title bar, tab strip, breadcrumb.
const CHROME_H: f32 = 96.0;
/// Production's Integrations section holds ONE row (`claude-history`). A
/// section several viewports tall scrolls internally and masks a short scroll
/// extent in the panel above it.
const INTEGRATION_ROWS: usize = 1;
const SIDEBAR_W: f32 = 200.0;
const SIDEBAR_BLOCK: &str = "block:default-left-sidebar";
const SIDEBAR_LOCAL: &str = "default-left-sidebar";

/// Pull the `left_sidebar::render::0` SRC block body out of the seed org.
fn extract_sidebar_render() -> String {
    let start = "#+BEGIN_SRC render :id left_sidebar::render::0";
    let mut body = Vec::new();
    let mut in_block = false;
    for line in SEED_ORG.lines() {
        if in_block {
            if line.trim_start().starts_with("#+END_SRC") {
                break;
            }
            body.push(line);
        } else if line.contains(start) {
            in_block = true;
        }
    }
    assert!(
        !body.is_empty(),
        "the seed must contain a `{start}` render block — did the id change?"
    );
    body.join("\n")
}

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

/// The sidebar block tree AS PRODUCTION BUILDS IT: the seeded
/// `column(tree(...), divider(), row(... "Integrations" ...), live_query(...))`
/// interpreted by the production shadow interpreter, with the (engine-less,
/// empty) `tree` collection swapped for a tall static page collection so the
/// content overflows the viewport. The whole column is wrapped in the
/// `view_mode_switcher` the backend adds to a block carrying both a query
/// source and a render source — `render_entity('block:default-left-sidebar')
/// OK: render="view_mode_switcher"` in Martin's live log.
fn sidebar_block_tree(
    services: &Arc<dyn holon_frontend::reactive::BuilderServices>,
) -> ReactiveViewModel {
    let expr =
        parse_render_dsl(&extract_sidebar_render()).expect("seeded left-sidebar render must parse");
    let interp = holon_frontend::shadow_builders::build_shadow_interpreter();
    let seeded_column = interp.interpret(&expr, &RenderContext::default(), &**services);
    assert_eq!(
        seeded_column.widget_name().as_deref(),
        Some("column"),
        "the seeded left sidebar must still be a `column(...)`"
    );
    assert!(
        seeded_column
            .children
            .iter()
            .any(|c| c.widget_name().as_deref() == Some("live_query")),
        "the seeded sidebar column must still carry the Integrations `live_query` \
         as a trailing sibling — that trailing sibling is the tail under test"
    );

    let pages = Arc::new(ReactiveView::new_static_with_layout(
        (0..PAGE_COUNT).map(page_row).collect::<Vec<_>>(),
        CollectionVariant::list(0.0),
    ));
    let tall_tree = ReactiveViewModel {
        collection: Some(pages),
        ..ReactiveViewModel::from_widget("list", HashMap::new())
    };

    let mut children: Vec<Arc<ReactiveViewModel>> = vec![Arc::new(tall_tree)];
    children.extend(seeded_column.children.iter().skip(1).cloned());
    let column = ReactiveViewModel {
        children,
        ..seeded_column
    };

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
        slot: Some(ReactiveSlot::new(column)),
        ..ReactiveViewModel::from_widget("view_mode_switcher", props)
    }
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

fn rows_with_prefix(bounds: &BoundsRegistry, prefix: &str) -> Vec<ElementInfo> {
    let mut rows: Vec<ElementInfo> = bounds
        .all_elements()
        .into_iter()
        .map(|(_, i)| i)
        .filter(|i| {
            i.widget_type.as_ref() == "text"
                && i.entity_id
                    .as_deref()
                    .is_some_and(|e| e.starts_with(prefix))
        })
        .collect();
    rows.sort_by(|a, b| a.y.total_cmp(&b.y));
    rows
}

/// The sidebar column's own box (`drawer::render`'s `h_full` outer div, tracked
/// as `drawer_toggle`'s sibling — the toggle is `h_full` too, so it measures
/// the drawer's full height).
fn drawer_band(bounds: &BoundsRegistry) -> ElementInfo {
    bounds
        .all_elements()
        .into_iter()
        .map(|(_, i)| i)
        .find(|i| i.widget_type.as_ref() == "drawer_toggle")
        .expect("the left drawer must render its toggle")
}

/// Scroll the sidebar to its maximum: repeated large wheels over the sidebar
/// band, so the assertion is about the scroll EXTENT, not one wheel's delta.
#[gpui::test]
fn left_sidebar_scrolls_to_its_last_row(cx: &mut TestAppContext) {
    cx.update(|cx| gpui_component::init(cx));

    let registry = Arc::new(BlockTreeRegistry::new());
    let test_services = support::TestServices::with_registry_quiescent(registry.clone());
    test_services.set_live_query_rows(INTEGRATION_ROWS);
    let services: Arc<dyn holon_frontend::reactive::BuilderServices> = test_services;
    {
        let services = services.clone();
        let thunk: BlockTreeThunk = Arc::new(move || sidebar_block_tree(&services));
        registry.register(SIDEBAR_BLOCK, vec![("default".to_string(), thunk)], 0);
    }

    let window_size = size(px(WINDOW_W), px(WINDOW_H));
    let bounds = BoundsRegistry::new();
    let window: WindowHandle<ReactiveFixtureView> = cx.update(|cx| {
        let bounds = bounds.clone();
        let services = services.clone();
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: Point::default(),
                    size: window_size,
                })),
                ..Default::default()
            },
            |_window, cx| {
                cx.new(|_cx| {
                    ReactiveFixtureView::with_services_and_bounds(
                        root(),
                        services,
                        window_size,
                        bounds,
                    )
                    .with_page_chrome(CHROME_H)
                })
            },
        )
        .expect("open_window failed")
    });
    let vcx = &mut VisualTestContext::from_window(window.into(), cx);
    vcx.run_until_parked();

    for _ in 0..16 {
        simulate_wheel_at(
            vcx,
            point(px(SIDEBAR_W / 2.0), px(WINDOW_H / 2.0)),
            px(-2000.0),
        );
        vcx.run_until_parked();
    }
    bounds.flush();

    // The sidebar's own viewport must fit inside the window in the first
    // place — a scroll region taller than the window can never bring its
    // bottom rows on screen, however far it scrolls.
    let band = drawer_band(&bounds);
    assert!(
        band.y + band.height <= WINDOW_H,
        "SIDEBAR VIEWPORT OVERFLOWS THE WINDOW: the drawer spans y={}..{} but \
         the window is only {WINDOW_H} px tall — it hangs {} px below the bottom \
         edge. A scroll region taller than the window can never bring its last \
         rows on screen, however far it scrolls.",
        band.y,
        band.y + band.height,
        band.y + band.height - WINDOW_H,
    );

    let pages = rows_with_prefix(&bounds, "page-");
    assert!(
        !pages.is_empty(),
        "the sidebar must render page rows at all"
    );
    let integrations = rows_with_prefix(&bounds, "integration-");

    // The LAST authored row in the sidebar column is the last Integrations
    // row. At scroll maximum it must be fully inside the window.
    let last = integrations.last().unwrap_or_else(|| {
        panic!(
            "SIDEBAR TAIL UNREACHABLE: at scroll maximum not one Integrations row \
             is laid out.\nvisible page rows: {:?}",
            pages
                .iter()
                .map(|p| (p.entity_id.clone(), p.y, p.height))
                .collect::<Vec<_>>()
        )
    });
    assert!(
        last.height > 0.0 && last.y + last.height <= WINDOW_H,
        "SIDEBAR TAIL UNREACHABLE: at scroll maximum the last Integrations row \
         `{:?}` sits at y={} h={} — its bottom ({}) is past the window bottom \
         ({WINDOW_H}).\nIntegrations rows: {:?}",
        last.entity_id,
        last.y,
        last.height,
        last.y + last.height,
        integrations
            .iter()
            .map(|s| (s.entity_id.clone(), s.y, s.height))
            .collect::<Vec<_>>()
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;

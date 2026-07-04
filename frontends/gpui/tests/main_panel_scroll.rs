//! Native (non-virtualized) scroll regression for the main panel.
//!
//! The seeded main panel renders `column(collection_view(), divider(),
//! "Linked references", live_query(...))`. Because the outline collection is
//! stacked inside a `column`, `column.rs` renders it EAGERLY at CONTENT height
//! (`eager_collection_div` / `view_mode_switcher::render_content_height`) — a
//! plain `div().flex_col().w_full()` with no `gpui::list`, hence no `ListState`
//! and no self-scrolling. Such content can only scroll if an ANCESTOR is a
//! definite-height `overflow_y_scroll` viewport.
//!
//! `columns::render`'s shrink-drawer (sidebar) branch already wraps its panel
//! in `h_full().overflow_y_scroll()`. The flow-panel (main panel) branch did
//! NOT — it historically relied on a self-scrolling `gpui::list` descendant.
//! Once the main panel switched to the eager content-height column, the
//! overflowing outline was simply clipped by the root `overflow_hidden` and
//! wheel / trackpad events no-oped: the main panel stopped scrolling (dogfood
//! 2026-07-22, reported by Martin).
//!
//! The ListState-based scroll tests (`layout_scroll.rs`,
//! `panel_scroll_spike.rs`) structurally cannot see this — they drive
//! `ListState::scroll_by`, which the eager path has none of. This test drives a
//! REAL wheel event through the production `columns::render` path and observes,
//! via `BoundsRegistry`, that a below-the-fold row (clipped to zero visible
//! height until then) is revealed — which only happens if the flow panel is a
//! working scroll viewport.
//!
//! Run: `cargo test -p holon-gpui --test main_panel_scroll`

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
use holon_gpui::geometry::BoundsRegistry;
use support::ReactiveFixtureView;
use support::simulate_wheel_at;

const VIEWPORT_W: f32 = 600.0;
const VIEWPORT_H: f32 = 400.0;
/// Enough rows that the content is several viewports tall — far rows start
/// clipped below the fold.
const ITEM_COUNT: usize = 80;
/// A row well past the first viewport (~11-16 rows fit 400px).
const FAR_IX: usize = 60;

fn item_id(ix: usize) -> String {
    format!("test-item-{ix}")
}

/// A `text` row whose production render path records bounds keyed by `data.id`
/// (`builders/text.rs`) — the smallest construction that puts per-row
/// `entity_id`s into `BoundsRegistry` (mirrors
/// `panel_scroll_spike::text_item`).
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

/// `columns( column( <list-collection> ) )` — the production shape of a flow
/// (main) panel whose content is an eager, content-height collection. The
/// `column` wrapper is what forces `column.rs`'s content-height eager render
/// (a bare list-collection under `columns` would instead take the virtualized
/// `gpui::list` path). The `column` is the `columns` flow child, so it renders
/// through `columns::render`'s flow-panel wrapper — the one that must be a
/// scroll viewport.
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

/// Visible (content-mask-clipped) height a row committed to the registry, or
/// `None` if the row was never tracked. A row fully below the fold clips to
/// zero visible height.
fn visible_height(registry: &BoundsRegistry, entity_id: &str) -> Option<f32> {
    registry
        .all_elements()
        .iter()
        .find(|(_, info)| info.entity_id.as_deref() == Some(entity_id))
        .map(|(_, info)| info.height)
}

/// RED before the fix / GREEN after: a wheel event over the main panel scrolls
/// its eager content-height column, revealing a row that started below the
/// fold. Before the fix the flow-panel wrapper had no `overflow_y_scroll`
/// viewport, so the wheel no-oped and the far row stayed clipped to 0 height.
#[gpui::test]
fn wheel_scrolls_eager_content_height_main_panel(cx: &mut TestAppContext) {
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

    // Sanity: the top row is visible and the far row starts clipped (below the
    // fold) — otherwise this isn't a scroll scenario and the test is vacuous.
    let top_before = visible_height(&bounds, &item_id(0)).unwrap_or(0.0);
    let far_before = visible_height(&bounds, &item_id(FAR_IX)).unwrap_or(0.0);
    assert!(
        top_before > 0.0,
        "top row should be visible before scrolling (got height {top_before})"
    );
    assert!(
        far_before <= 0.0,
        "row {FAR_IX} should start below the fold (clipped to 0 visible height) but had \
         height {far_before} — pick a farther row or a shorter viewport"
    );

    // Real wheel over the main panel centre. `simulate_wheel_at` sends a
    // preceding mouse-move so the hit-test lands on the scroll hitbox.
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
        "after a wheel-scroll down, row {FAR_IX} should have scrolled into view (nonzero visible \
         height) but was still clipped (height {far_after}). The main-panel flow wrapper in \
         `columns::render` is not a scroll viewport — the eager content-height column cannot \
         scroll, so wheel / trackpad events no-op. Add `.overflow_y_scroll()` to the flow-panel \
         wrapper (matching the shrink-drawer sidebar branch)."
    );
}

// Installs the windowed capturing tracing subscriber before this binary's
// first line of test code (see tests/test_init/mod.rs).
mod test_init;

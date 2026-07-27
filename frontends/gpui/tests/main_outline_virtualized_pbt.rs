//! Inc 5 (vault "Rule Inc 5 go/no-go: restore gpui::list virtualization for the
//! main outline"; Keystone "Add a virtualized content-height collection mode
//! before large-outline vaults" — "Zero-height fix renders eagerly: O(N)
//! rows/frame").
//!
//! The zero-height fix (feb8a4e5a2, BugFunnel 230/232 + 2026-07-22) made every
//! column-nested collection render EAGERLY at content height
//! (`column.rs::eager_collection_div`): every row is built + laid out every
//! frame. For the main outline that is O(N) rows/frame — a 2000-block page
//! prepaints 2000 rows on every render pass, blowing the p95 latency SLO.
//!
//! This windowed PBT pins the per-frame render cost. Observable: how many
//! DISTINCT row entity-ids commit bounds into `BoundsRegistry` in a settled,
//! steady-state (non-first) render pass. `TransparentTracker::prepaint` records
//! one entry per laid-out element, so the count of distinct row ids == the
//! number of rows the frame actually built.
//!
//!  - EAGER (current): every row is built every frame → ~N distinct ids → RED.
//!  - VIRTUALIZED (target): only viewport ± overdraw rows are built per steady
//!    frame → O(viewport) distinct ids, well under N → GREEN.
//!
//! A second rung proves the below-fold contract still holds: scrolling reveals
//! a row that started below the fold (reuses the `simulate_wheel_at` pump path
//! the MCP scroll tests exercise). Under virtualization that row is not even
//! built until scrolled into range — after the wheel it must be built AND
//! visible, so the wheel/settle correctness contract the PBT drive layer relies
//! on survives the change.
//!
//! Run: `cargo test -p holon-gpui --test main_outline_virtualized_pbt`

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::collections::HashSet;
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
const VIEWPORT_H: f32 = 600.0;
/// Large enough that O(N) eager rendering is unmistakably above any plausible
/// O(viewport)+overdraw count (600px viewport + 2×1000px overdraw ≈ 2600px of
/// rows; even at a 6px row height that is < 450 rows).
const ITEM_COUNT: usize = 1200;
/// Ceiling for the O(viewport) claim. Comfortably above viewport+overdraw,
/// comfortably below `ITEM_COUNT` — the whole point of virtualization.
const VIEWPORT_ROW_CEILING: usize = 600;

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

/// `columns( column( <list-collection> ) )` — the production shape of the eager
/// content-height main panel (identical to `mcp_scroll_wheel_eager_panel.rs`'s
/// fixture, which documents why the `column` wrapper forces the eager
/// `overflow_y_scroll` render with no `ListState`).
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

/// Distinct row entity-ids that committed bounds in the last render pass.
fn distinct_rows_rendered(registry: &BoundsRegistry) -> HashSet<String> {
    registry
        .all_elements()
        .iter()
        .filter_map(|(_, info)| info.entity_id.as_deref())
        .filter(|id| id.starts_with("test-item-"))
        .map(|id| id.to_string())
        .collect()
}


#[ignore = "red-for-the-right-reason: eager path builds O(N) rows/frame; un-ignore with the Inc 5 virtualization implementation (needs Martin ruling: Integrations pin-vs-scroll)"]
#[gpui::test]
fn main_outline_renders_o_viewport_rows_per_frame(cx: &mut TestAppContext) {
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

    // Force a second, steady-state render pass. A virtualized list measures all
    // rows ONCE on first layout (O(N) that frame only); every later frame is
    // O(viewport). A small wheel nudge guarantees the pass we measure is a
    // post-measurement steady-state pass — the honest per-frame cost. The eager
    // path re-builds every row on this pass too, so it stays RED.
    simulate_wheel_at(
        vcx,
        point(px(VIEWPORT_W / 2.0), px(VIEWPORT_H / 2.0)),
        px(-40.0),
    );
    vcx.run_until_parked();
    bounds.flush();

    let rendered = distinct_rows_rendered(&bounds);
    eprintln!(
        "[virt-pbt] distinct rows built in steady-state frame = {} (N = {ITEM_COUNT})",
        rendered.len()
    );
    assert!(
        rendered.len() <= VIEWPORT_ROW_CEILING,
        "main outline must render O(viewport) rows per frame, not O(N): built {} distinct rows \
         for an {ITEM_COUNT}-row outline (ceiling {VIEWPORT_ROW_CEILING}). The eager \
         column-nested path (`column.rs::eager_collection_div`) builds every row every frame; \
         restore `gpui::list` virtualization for the main outline.",
        rendered.len()
    );
}

/// Highest row index currently visible (nonzero committed height). `None` when
/// no row is visible.
fn max_visible_row_ix(registry: &BoundsRegistry) -> Option<usize> {
    registry
        .all_elements()
        .iter()
        .filter(|(_, info)| info.height > 0.0)
        .filter_map(|(_, info)| info.entity_id.as_deref())
        .filter_map(|id| id.strip_prefix("test-item-"))
        .filter_map(|n| n.parse::<usize>().ok())
        .max()
}

/// Correctness rung: the below-fold reveal contract the PBT drive layer relies
/// on must survive virtualization. Under virtualization a below-fold row is not
/// even built until scrolled into range — so a bounded wheel scroll must
/// advance the visible window to include rows further down the outline. Written
/// geometry-robustly (assert the visible window advances) so it stays green in
/// both the eager world (today) and the virtualized world (post-fix), without
/// depending on the exact row height. Bounded scroll (well short of the bottom
/// for N=1200) so we never over-scroll past the rows we expect to reveal.
#[gpui::test]
fn scrolling_reveals_below_fold_outline_row(cx: &mut TestAppContext) {
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

    let top_max = max_visible_row_ix(&bounds).expect("some rows visible on first frame");

    // A single bounded wheel nudge — enough to advance the fold by a few rows,
    // far short of the bottom (N=1200) so the newly-revealed rows are genuinely
    // "below the fold" content, not the clamped tail.
    simulate_wheel_at(
        vcx,
        point(px(VIEWPORT_W / 2.0), px(VIEWPORT_H / 2.0)),
        px(-1600.0),
    );
    vcx.run_until_parked();
    bounds.flush();

    let scrolled_max = max_visible_row_ix(&bounds).expect("some rows visible after scroll");
    assert!(
        scrolled_max > top_max,
        "scrolling must reveal below-fold rows: highest visible row index went from {top_max} to \
         {scrolled_max} (expected strictly greater). The wheel/settle path the MCP scroll tests \
         and the PBT drive layer rely on must keep advancing the visible window."
    );
}

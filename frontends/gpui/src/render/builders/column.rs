use holon_frontend::ReactiveViewModel;
use holon_frontend::reactive_view::ReactiveView;

use super::prelude::*;

/// True if `c` contributes a scrollable collection that a stacking `column`
/// must render at CONTENT height (see the loop below for why). Either the node
/// is itself a collection (`tree`/`list`/`live_query`), or it is a
/// `view_mode_switcher` whose slot holds the block's collection outline
/// (`collection_view()` composed inside `column(...)`).
fn holds_collection(c: &ReactiveViewModel) -> bool {
    c.collection.is_some() || vms_slot_collection(c).is_some()
}

/// The collection backing a `view_mode_switcher` node's slot, if this node is
/// one and its slot content is a collection.
fn vms_slot_collection(c: &ReactiveViewModel) -> Option<std::sync::Arc<ReactiveView>> {
    if c.widget_name().as_deref() != Some("view_mode_switcher") {
        return None;
    }
    let slot = c.slot.as_ref()?;
    let content = slot.content.lock_ref();
    content.collection.clone()
}

/// Render a scrollable collection at CONTENT height (eager, non-virtualized).
///
/// A `tree`/`list`/`live_query` stacked inside a `column` among fixed siblings
/// must render at CONTENT height, not through the `scrollable_list_wrapper`'s
/// `size_full` viewport. In a stacking column the column is content-sized
/// (indefinite height) — and it may itself sit inside an absolute-positioned
/// wrapper (the left sidebar's `view_mode_switcher`, or the main panel) — so a
/// `size_full`/`h_full` viewport resolves to 0 and the virtualized
/// `gpui::list` paints nothing. That was the left-sidebar page-tree blank bug
/// (BugFunnel rows 230 + 232) and the main-panel `collection_view()` 0-height
/// bug (BugFunnel 2026-07-22).
///
/// Rows are built eagerly from the collection snapshot every frame.
/// Reactivity is preserved: the enclosing block-mode `ReactiveShell`
/// subscribes to every nested collection's `MutableVec`
/// (`walk_for_collections`/`collection_subs`), so a data change re-renders the
/// parent and this loop re-reads a fresh snapshot.
///
/// TRADEOFF (accepted): this eager path is NON-virtualized — every row is built
/// each frame. Fine for the sidebar's handful of pages / the focused page's
/// outline. Follow-up if a large collection ever gets column-wrapped: a
/// content-height *virtualized* mode in `ReactiveShell` (Infer sizing).
///
/// Collapse filtering is applied here (the virtualized list did it via
/// `ReactiveShell::compute_visible_indices`): descendants of a collapsed
/// `tree_item` are skipped. `tree_item_collapse_state` returns `None` for flat
/// rows (e.g. the sync-states `live_query`), so those always render.
pub(crate) fn eager_collection_div(view: &ReactiveView, ctx: &GpuiRenderContext) -> Div {
    let mut list_div = div().flex().flex_col().w_full();
    let mut skip_below: Option<usize> = None;
    for item in view.children_snapshot() {
        if let Some((depth, collapsed)) = super::tree_item_collapse_state(item.as_ref(), ctx) {
            if let Some(threshold) = skip_below {
                if depth > threshold {
                    continue;
                }
                skip_below = None;
            }
            if collapsed {
                skip_below = Some(depth);
            }
        }
        list_div = list_div.child(super::render(item.as_ref(), ctx));
    }
    list_div
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let gap = node.prop_f64("gap").unwrap_or(0.0) as f32;
    let children = &node.children;

    // Content-only columns render through the original container unchanged
    // (`div().flex().flex_col()`). A column that stacks a scrollable
    // collection additionally pins `w_full` so the eagerly-rendered rows fill
    // the sidebar width. Gating on the collection avoids widening the many
    // content-only `column(...)`s (block templates, widget gallery, list item
    // templates) that must keep their intrinsic cross-axis width.
    let has_collection_child = children.iter().any(|c| holds_collection(c));

    let mut container = div().flex().flex_col();
    if has_collection_child {
        container = container.w_full();
    }
    if gap > 0.0 {
        container = container.gap(px(gap));
    }
    for child in children {
        if let Some(view) = child.collection.as_ref() {
            container = container.child(eager_collection_div(view, ctx));
        } else if vms_slot_collection(child).is_some() {
            // `collection_view()` composed inside `column(...)` expands to a
            // `view_mode_switcher` whose slot holds the outline collection. Its
            // default (`size_full`, absolutely-positioned slot content) render
            // path collapses to 0 height in a content-sized column — the exact
            // 2026-07-22 main-panel bug. Render it content-height instead,
            // keeping the mode-switcher chrome as an overlay.
            container =
                container.child(super::view_mode_switcher::render_content_height(child, ctx));
        } else {
            container = container.child(super::render(child, ctx));
        }
    }
    container
}

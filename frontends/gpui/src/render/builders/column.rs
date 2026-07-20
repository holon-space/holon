use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let gap = node.prop_f64("gap").unwrap_or(0.0) as f32;
    let children = &node.children;

    // Content-only columns render through the original container unchanged
    // (`div().flex().flex_col()`). A column that stacks a scrollable
    // collection additionally pins `w_full` so the eagerly-rendered rows fill
    // the sidebar width. Gating on the collection avoids widening the many
    // content-only `column(...)`s (block templates, widget gallery, list item
    // templates) that must keep their intrinsic cross-axis width.
    let has_collection_child = children.iter().any(|c| c.collection.is_some());

    let mut container = div().flex().flex_col();
    if has_collection_child {
        container = container.w_full();
    }
    if gap > 0.0 {
        container = container.gap(px(gap));
    }
    for child in children {
        if let Some(view) = child.collection.as_ref() {
            // A scrollable collection (a `tree`/`list`/`live_query` child)
            // stacked inside a `column` among fixed siblings must render at
            // CONTENT height, not through the `scrollable_list_wrapper`'s
            // `size_full` viewport. In a stacking column the column is
            // content-sized (indefinite height) — and, in the left sidebar,
            // the `column` sits inside the `view_mode_switcher`'s
            // absolute-positioned wrapper — so a `size_full`/`h_full`
            // viewport resolves to 0 and the virtualized `gpui::list` paints
            // nothing. That was the left-sidebar page-tree blank-render bug
            // (BugFunnel rows 230 + 232): before the Integrations section
            // wrapped the tree in a `column`, the tree was the drawer's sole
            // full-height child and `size_full` resolved to the definite
            // drawer height. The drawer already scrolls (`overflow_y_scroll`),
            // so stacked sections just need their natural height.
            //
            // Rows are built eagerly from the collection snapshot every frame.
            // Reactivity is preserved: the enclosing block-mode `ReactiveShell`
            // subscribes to every nested collection's `MutableVec`
            // (`walk_for_collections`/`collection_subs`), so a data change
            // re-renders the parent and this loop re-reads a fresh snapshot.
            //
            // TRADEOFF (accepted): this eager path is NON-virtualized — every
            // row is built each frame. Fine for the sidebar's handful of pages
            // / integrations. Follow-up if a large collection ever gets
            // column-wrapped: a content-height *virtualized* mode in
            // `ReactiveShell` (Infer sizing) instead of the size_full list.
            //
            // Collapse filtering is applied here (the virtualized list did it
            // via `ReactiveShell::compute_visible_indices`): descendants of a
            // collapsed tree_item are skipped. `tree_item_collapse_state`
            // returns `None` for flat rows (e.g. the sync-states `live_query`),
            // so those always render.
            let mut list_div = div().flex().flex_col().w_full();
            let mut skip_below: Option<usize> = None;
            for item in view.children_snapshot() {
                if let Some((depth, collapsed)) =
                    super::tree_item_collapse_state(item.as_ref(), ctx)
                {
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
            container = container.child(list_div);
        } else {
            container = container.child(super::render(child, ctx));
        }
    }
    container
}

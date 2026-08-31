use holon_frontend::LayoutHint;
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

/// True if `c` declares that it belongs at its container's trailing edge,
/// outside the scrolling region — the trigger for the flow-panel split. The
/// child declares this in its `LayoutHint`; nothing here knows or cares which
/// widget it is.
fn is_pinned_to_end(c: &ReactiveViewModel) -> bool {
    c.layout_hint == LayoutHint::PinnedToEnd
}

/// True if `node` has ≥1 direct child pinned to its trailing edge — the trigger
/// for the flow-panel split (plan §4). A container without such a child takes
/// the byte-identical original path (sidebar firewall: sidebar columns hold no
/// pinned child, so they never reach the split).
pub(crate) fn has_pinned_child(node: &ReactiveViewModel) -> bool {
    node.children.iter().any(|c| is_pinned_to_end(c))
}

/// The pin-bearing container hiding one slot below `node`, if `node` is a
/// slot-bearing wrapper around one.
///
/// The backend wraps a panel whose block has BOTH a query source and a render
/// source in the query-source switcher (`block_domain.rs`,
/// `wrap_in_query_source_switcher`), so production's panel-tree root is a
/// `view_mode_switcher` and the authored `column` sits in its slot — the shape
/// both sidebars already have. A slot renders exactly ONE content node, so it
/// changes nothing about where that content sits: splitting the slot's
/// container is the same split, one level down. Without this,
/// [`has_pinned_child`] is false for the real seeded main panel and its
/// accordion is never pinned.
pub(crate) fn slot_pinned_container(
    node: &ReactiveViewModel,
) -> Option<std::sync::Arc<ReactiveViewModel>> {
    let content = node.slot.as_ref()?.content.get_cloned();
    has_pinned_child(&content).then_some(content)
}

/// True when `node` is a main-panel flow `column` that hosts the scrollable
/// outline — it carries either a pinned footer OR a scrollable collection
/// (`tree`/`list`/`live_query`/`collection_view`). Such columns render through
/// [`render_accordion_split`] so the outline VIRTUALIZES (`gpui::list`, only
/// viewport rows/frame) while fixed sections pin. The sidebar's column reaches
/// [`render`]'s eager content-height path instead (the blank-panel firewall,
/// BugFunnel 230/232) and is never routed here — only Flex flow panels / the
/// pin-bearing block-mode arm are.
pub(crate) fn is_main_panel_flow_column(node: &ReactiveViewModel) -> bool {
    node.widget_name().as_deref() == Some("column")
        && node
            .children
            .iter()
            .any(|c| is_pinned_to_end(c) || holds_collection(c))
}

#[cfg(test)]
mod split_target_tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use holon_frontend::LayoutHint;
    use holon_frontend::ReactiveViewModel;
    use holon_frontend::reactive_view_model::ReactiveSlot;

    use super::has_pinned_child;
    use super::slot_pinned_container;

    fn node(widget: &str, children: Vec<ReactiveViewModel>) -> ReactiveViewModel {
        ReactiveViewModel {
            children: children.into_iter().map(Arc::new).collect(),
            ..ReactiveViewModel::from_widget(widget, HashMap::new())
        }
    }

    /// An accordion the shadow layer accepted: its container offered
    /// `PinToEnd`, so it declares the pin.
    fn pinned_accordion() -> ReactiveViewModel {
        ReactiveViewModel {
            layout_hint: LayoutHint::PinnedToEnd,
            ..node("accordion", vec![])
        }
    }

    /// An accordion whose container could NOT honour the pin — the shadow
    /// builder returned the fail-loud placement error, which declares no pin.
    fn misplaced_accordion() -> ReactiveViewModel {
        node("error", vec![])
    }

    fn switcher_over(slot: ReactiveViewModel) -> ReactiveViewModel {
        ReactiveViewModel {
            slot: Some(ReactiveSlot::new(slot)),
            ..ReactiveViewModel::from_widget("view_mode_switcher", HashMap::new())
        }
    }

    #[test]
    fn switcher_over_pinning_column_resolves_to_that_column() {
        let tree = switcher_over(node("column", vec![pinned_accordion()]));
        let column = slot_pinned_container(&tree).expect("the slot column must be found");
        assert!(has_pinned_child(&column));
    }

    /// The sidebar firewall: both sidebars are switcher-wrapped columns today
    /// and must keep taking the eager content-height path, never the split.
    /// They hold no pin-declaring child, so there is nothing sidebar-specific
    /// to exclude.
    #[test]
    fn switcher_over_plain_column_is_not_a_split_target() {
        let tree = switcher_over(node("column", vec![node("list", vec![])]));
        assert!(slot_pinned_container(&tree).is_none());
    }

    /// Mode switched to `source`: the slot holds the query editor, so the split
    /// must stop firing until the switcher goes back to the result mode.
    #[test]
    fn switcher_over_non_container_is_not_a_split_target() {
        let tree = switcher_over(node("source_editor", vec![]));
        assert!(slot_pinned_container(&tree).is_none());
    }

    /// An accordion buried in a `row` never gets the pin offered, so the shadow
    /// layer replaced it with the placement error — nothing declares a pin and
    /// no split fires.
    #[test]
    fn switcher_over_row_wrapped_accordion_is_not_a_split_target() {
        let tree = switcher_over(node("row", vec![misplaced_accordion()]));
        assert!(slot_pinned_container(&tree).is_none());
    }

    #[test]
    fn a_bare_column_is_not_a_slot_target() {
        let tree = node("column", vec![pinned_accordion()]);
        assert!(has_pinned_child(&tree));
        assert!(slot_pinned_container(&tree).is_none());
    }
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
    // A `horizontal` collection (an integration row's op buttons) lays its
    // items along one baseline at content width, so every item sits on its
    // row's line. The default stacks them full-width — what a page tree or
    // outline wants.
    let layout = view.layout();
    let mut list_div = if layout.as_ref().is_some_and(|l| l.horizontal) {
        div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(layout.as_ref().map(|l| l.gap).unwrap_or(0.0)))
    } else {
        div().flex().flex_col().w_full()
    };
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

/// Append one column child to `container`, routing it to the correct
/// content-height path: a collection renders eagerly, a `view_mode_switcher`
/// slot renders content-height, everything else renders normally. Shared by
/// `render`, the accordion split's main body, and the accordion body so the
/// routing lives in exactly one place.
fn push_content_child(container: Div, child: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    if let Some(view) = child.collection.as_ref() {
        container.child(eager_collection_div(view, ctx))
    } else if vms_slot_collection(child).is_some() {
        // `collection_view()` composed inside `column(...)` expands to a
        // `view_mode_switcher` whose slot holds the outline collection. Its
        // default (`size_full`, absolutely-positioned slot content) render path
        // collapses to 0 height in a content-sized column — the exact
        // 2026-07-22 main-panel bug. Render it content-height instead, keeping
        // the mode-switcher chrome as an overlay.
        container.child(super::view_mode_switcher::render_content_height(child, ctx))
    } else if child.slot.is_some() {
        // A slot node (`live_query`, `live_block`) mints a `ReactiveShell` at
        // `ctx.placement`, and the `Panel` shape claims `height: relative(1.0)`.
        // This container is content-sized, so that percentage has no definite
        // parent, resolves to 0 px, and takes the whole section with it. The
        // class is routed here rather than a per-widget list, so a new
        // shell-bearing widget cannot silently reintroduce the 0-px collapse.
        container.child(super::render(child, &ctx.nested()))
    } else {
        container.child(super::render(child, ctx))
    }
}

/// Append one MAIN-region child, VIRTUALIZING the outline collection. The
/// outline (a bare collection, or a `collection_view` whose slot holds one)
/// renders through the nested collection-mode `ReactiveShell` + `gpui::list`
/// (only viewport rows built per frame) inside a `flex_1 min_h_0` region that
/// owns its own scroll — NO outer `overflow_y_scroll` double-wrap. Non-
/// collection siblings (divider, section header) stay content-height and pin
/// (`flex_shrink_0`). Contrast [`push_content_child`], the eager sidebar path.
fn push_main_child(container: Div, child: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    if child.collection.is_some() {
        container.child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(super::render(child, ctx)),
        )
    } else if vms_slot_collection(child).is_some() {
        container.child(
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(super::view_mode_switcher::render_virtualized(child, ctx)),
        )
    } else {
        container.child(
            div()
                .flex_shrink_0()
                .w_full()
                .child(super::render(child, ctx)),
        )
    }
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
    // A `min_height` floor keeps a short/empty section (a LogSeq journal day)
    // occupying a comfortable block; taller content grows past it.
    if let Some(mh) = node.prop_f64("min_height") {
        container = container.min_h(px(mh as f32));
    }
    for child in children {
        container = push_content_child(container, child, ctx);
    }
    container
}

/// Render the column's NON-accordion children as the VIRTUALIZED main body that
/// fills the `flex_1 min_h_0` main region of the accordion split. The outline
/// collection renders through the nested collection-mode `ReactiveShell` +
/// `gpui::list` (via `push_main_child`); the accordion(s) become the pinned
/// footer. Non-collection siblings (divider, header) stay content-height.
fn render_main_body(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let gap = node.prop_f64("gap").unwrap_or(0.0) as f32;

    // Fill the split's `flex_1 min_h_0` main region so the virtualized outline
    // inside `push_main_child` inherits a DEFINITE viewport height (the
    // condition `gpui::list` needs for a nonzero `scroll_max`).
    let mut container = div().flex().flex_col().flex_1().min_h_0().w_full();
    if gap > 0.0 {
        container = container.gap(px(gap));
    }
    for (ix, child) in node.children.iter().enumerate() {
        if is_pinned_to_end(child) || introduces_nothing(node, ix, ctx) {
            continue;
        }
        container = push_main_child(container, child, ctx);
    }
    container
}

/// True for a divider whose whole remainder is pinned children that this frame
/// does not paint — a full-width rule introducing nothing. The split asks the
/// footer itself whether it is painting; the divider knows nothing about rows.
fn introduces_nothing(node: &ReactiveViewModel, ix: usize, ctx: &GpuiRenderContext) -> bool {
    if node.children[ix].widget_name().as_deref() != Some("divider") {
        return false;
    }
    let rest = &node.children[ix + 1..];
    !rest.is_empty()
        && rest
            .iter()
            .all(|c| is_pinned_to_end(c) && super::accordion::is_hidden(c, ctx))
}

/// Render a slice of children as a content-height `flex_col` (used for the
/// accordion body's eager content).
pub(crate) fn render_children_content_height(
    children: &[std::sync::Arc<ReactiveViewModel>],
    ctx: &GpuiRenderContext,
) -> Div {
    let mut container = div().flex().flex_col().w_full();
    for child in children {
        container = push_content_child(container, child, ctx);
    }
    container
}

/// The flow-panel accordion split (plan §4, Martin's R8 = PINNED FOOTER).
///
/// `inner` is the definite-height `absolute size_full` div from
/// `columns::panel_wrap`. We turn it into a `flex_col` whose:
///   - MAIN region (`flex_1 min_h_0`) holds the column's non-accordion
///     children; its outline collection renders VIRTUALIZED (`gpui::list` via
///     the nested collection-mode `ReactiveShell`, only viewport rows/frame)
///     and owns its own scroll — Inc 5 (`main_outline_virtualized_pbt`);
///   - FOOTER(s) are the bounded accordion(s), `flex_shrink_0`, PINNED at the
///     panel bottom — they never scroll with the outline (Martin's ruling:
///     fixed sections pin, they do not scroll with the outline).
/// `pad` is `(horizontal, vertical)` padding for the drawer-branch main panel
/// (`None` for the plain flow branch, which had no padding).
pub(crate) fn render_accordion_split(
    inner: Div,
    node: &ReactiveViewModel,
    scroll_id: ElementId,
    pad: Option<(f32, f32)>,
    ctx: &GpuiRenderContext,
) -> AnyElement {
    // MAIN region: `flex_1 min_h_0` gives the outline a definite viewport; the
    // virtualized `gpui::list` inside `render_main_body` OWNS its own scroll, so
    // there is deliberately NO `overflow_y_scroll` here (a double scroll
    // viewport would inflate the list's measured size and zero out scroll_max).
    let mut main = div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .id(scroll_id);
    if let Some((_, pad_y)) = pad {
        main = main.py(px(pad_y));
    }
    main = main.child(render_main_body(node, ctx));

    let mut wrapper = inner.flex().flex_col();
    if let Some((pad_x, _)) = pad {
        wrapper = wrapper.px(px(pad_x));
    }
    wrapper = wrapper.child(main);

    for child in &node.children {
        if is_pinned_to_end(child) {
            wrapper = wrapper.child(super::tag(
                ctx,
                "accordion",
                super::accordion::render_bounded(child, ctx),
            ));
        }
    }
    wrapper.into_any_element()
}

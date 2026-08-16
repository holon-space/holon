use std::sync::Arc;

use holon_frontend::LayoutHint;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::view_model::DrawerMode;

use super::prelude::*;

/// Wraps a rendered panel in the `flex_1 relative -> absolute size_full`
/// layout chain that hands a descendant scrollable list a definite viewport.
/// Without this chain the panel has no height constraint, `ReactiveShell`'s
/// list collapses to zero, and wheel events no-op.
///
/// `build_inner` receives the bare absolute div and returns a finished
/// element -- callers add id / overflow / padding / child at one site. The
/// closure returns `AnyElement` so it can wrap the div in `.id()` (which
/// produces `Stateful<Div>`) without the helper caring about the concrete
/// type.
fn panel_wrap(build_inner: impl FnOnce(Div) -> gpui::AnyElement) -> Div {
    let inner = div().absolute().top_0().left_0().size_full();
    div().flex_1().relative().child(build_inner(inner))
}

fn is_drawer(node: &ReactiveViewModel) -> bool {
    node.widget_name().as_deref() == Some("drawer")
}

fn drawer_mode(node: &ReactiveViewModel) -> DrawerMode {
    DrawerMode::from_str(
        &node
            .prop_str("mode")
            .unwrap_or_else(|| "shrink".to_string()),
    )
}

fn is_overlay_drawer(node: &ReactiveViewModel) -> bool {
    is_drawer(node) && matches!(drawer_mode(node), DrawerMode::Overlay)
}

fn is_shrink_drawer(node: &ReactiveViewModel) -> bool {
    is_drawer(node) && matches!(drawer_mode(node), DrawerMode::Shrink)
}

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let items: Vec<Arc<ReactiveViewModel>> = if let Some(ref view) = node.collection {
        view.items.lock_ref().iter().cloned().collect()
    } else {
        node.children.clone()
    };

    let gap = node
        .collection
        .as_ref()
        .and_then(|v| {
            v.layout()
                .as_ref()
                .filter(|l| l.name() == "columns")
                .map(|l| l.gap)
        })
        .unwrap_or(4.0);

    let has_overlay = items.iter().any(|n| is_overlay_drawer(n));
    let has_shrink_drawers = items.iter().any(|n| is_shrink_drawer(n));

    if !has_shrink_drawers && !has_overlay {
        let mut container = div()
            .flex()
            .flex_row()
            .items_stretch()
            .size_full()
            .gap(px(gap));
        for item in &items {
            let entity = item.entity();
            let scroll_id = entity
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or("panel");
            let id = hashed_id(scroll_id);
            // Main-panel flow-panel split: a Flex flow column carrying an
            // accordion child OR a scrollable collection (the outline) becomes a
            // VIRTUALIZED main region (gpui::list) + pinned footer. Every other
            // item takes the byte-identical original path below (sidebar
            // firewall — sidebars are Fixed/shrink-drawer, never Flex flow).
            if matches!(item.layout_hint, LayoutHint::Flex { .. })
                && super::column::is_main_panel_flow_column(item)
            {
                container = container.child(panel_wrap(move |inner| {
                    super::column::render_accordion_split(inner, item, id, None, ctx)
                }));
                continue;
            }
            let rendered = super::render(item, ctx);
            match item.layout_hint.fixed_px() {
                Some(fixed_px) => {
                    container = container.child(
                        div()
                            .flex_shrink_0()
                            .w(px(fixed_px))
                            .h_full()
                            .child(rendered),
                    );
                }
                None => {
                    // Same content-height scroll viewport as the drawer flow
                    // panel below (see the `overflow_y_scroll` note there):
                    // the flow child may render an eager content-height
                    // collection that must scroll within a definite viewport.
                    container = container.child(panel_wrap(move |inner| {
                        inner
                            .id(id)
                            .overflow_y_scroll()
                            .child(rendered)
                            .into_any_element()
                    }));
                }
            }
        }
        return container;
    }

    let mut container = div()
        .flex()
        .flex_row()
        .items_stretch()
        .size_full()
        .relative();

    let first_shrink = items.iter().position(|n| is_shrink_drawer(n));
    let last_shrink = items.iter().rposition(|n| is_shrink_drawer(n));

    let mut overlay_elements: Vec<(bool, AnyElement)> = Vec::new();
    let mut seen_flow_child = false;

    for (i, item) in items.iter().enumerate() {
        if is_overlay_drawer(item) {
            let is_right = seen_flow_child;
            overlay_elements.push((is_right, super::render(item, ctx)));
        } else if is_shrink_drawer(item) {
            let block_id = item.prop_str("block_id").unwrap_or_else(|| "".to_string());
            let prop_width = item.prop_f64("width").unwrap_or(300.0) as f32;
            let width =
                super::drawer::effective_drawer_width(&*ctx.services, &block_id, prop_width);
            let child = item.children.first();

            let is_first_shrink = Some(i) == first_shrink;
            let is_last_shrink = Some(i) == last_shrink && first_shrink != last_shrink;

            if is_first_shrink || is_last_shrink {
                let is_open = ctx.services.drawer_open(&block_id, DrawerMode::Shrink);

                let rendered = child.map(|c| super::render(c, ctx));
                let mut inner = div()
                    .id(hashed_id(&block_id))
                    .h_full()
                    .overflow_y_scroll()
                    .bg(tc(ctx, |t| t.sidebar))
                    .w(px(width))
                    .min_w(px(width))
                    .px(px(ctx.style().sidebar_padding_x))
                    .py(px(ctx.style().sidebar_padding_y))
                    .text_sm();
                if let Some(r) = rendered {
                    inner = inner.child(r);
                }
                inner = if is_first_shrink {
                    inner.border_r_1().border_color(tc(ctx, |t| t.border))
                } else {
                    inner.border_l_1().border_color(tc(ctx, |t| t.border))
                };

                // Clickable toggle widget at the canonical
                // `drawer_toggle_id_for(block_id)` element id. Stays
                // rendered even when closed so a user — or the layout
                // PBT's `ToggleDrawer` transition — can re-open the
                // drawer by clicking the same target. Placed at the
                // inner edge of the panel: for a `first_shrink`
                // (typically left sidebar) the toggle sits on the
                // right of the panel; for the `last_shrink` (right
                // sidebar) it sits on the left.
                let resize_anchor = if is_first_shrink {
                    super::drawer::ResizeAnchor::Left
                } else {
                    super::drawer::ResizeAnchor::Right
                };
                let tracked_toggle = super::drawer::drawer_toggle_widget(
                    &block_id,
                    "col",
                    DrawerMode::Shrink,
                    resize_anchor,
                    ctx,
                );
                let toggle_w = super::drawer::DRAWER_TOGGLE_WIDTH;

                // Container layout differs by open/closed: when open,
                // panel + toggle occupy `width + toggle_w`; when
                // closed, only the toggle is in flow — at the
                // appropriate edge for each sidebar position — so the
                // click target stays reachable.
                let column = if is_open {
                    let row = if is_first_shrink {
                        div()
                            .h_full()
                            .flex()
                            .flex_row()
                            .child(inner)
                            .child(tracked_toggle)
                    } else {
                        div()
                            .h_full()
                            .flex()
                            .flex_row()
                            .child(tracked_toggle)
                            .child(inner)
                    };
                    div()
                        .h_full()
                        .flex_shrink_0()
                        .w(px(width + toggle_w))
                        .child(row)
                } else {
                    div()
                        .h_full()
                        .flex_shrink_0()
                        .w(px(toggle_w))
                        .child(tracked_toggle)
                };

                container = container.child(column);
            } else {
                let rendered = child.map(|c| super::render(c, ctx));
                let id = hashed_id(&block_id);
                container = container.child(panel_wrap(|inner| {
                    let mut el = inner.id(id).overflow_y_scroll().p_2();
                    if let Some(r) = rendered {
                        el = el.child(r);
                    }
                    el.into_any_element()
                }));
            }
        } else {
            seen_flow_child = true;
            let entity = item.entity();
            let scroll_id = entity
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or("panel");
            let id = hashed_id(scroll_id);
            let pad_x = ctx.style().content_padding_x;
            let pad_y = ctx.style().content_padding_y;
            // Main-panel flow-panel split, drawer branch: same as the plain
            // flow branch (accordion OR collection outline) but padded.
            if matches!(item.layout_hint, LayoutHint::Flex { .. })
                && super::column::is_main_panel_flow_column(item)
            {
                container = container.child(panel_wrap(move |inner| {
                    super::column::render_accordion_split(
                        inner,
                        item,
                        id,
                        Some((pad_x, pad_y)),
                        ctx,
                    )
                }));
                continue;
            }
            let rendered = super::render(item, ctx);
            match item.layout_hint.fixed_px() {
                Some(fixed_px) => {
                    container = container.child(
                        div()
                            .flex_shrink_0()
                            .w(px(fixed_px))
                            .h_full()
                            .child(rendered),
                    );
                }
                None => {
                    container = container.child(panel_wrap(move |inner| {
                        inner
                            .id(id)
                            // The flow panel (main panel) content is now a
                            // CONTENT-height eager column (`column.rs`
                            // `eager_collection_div` / `view_mode_switcher::
                            // render_content_height`), not a self-scrolling
                            // `gpui::list`. Without an `overflow_y_scroll`
                            // viewport here the overflowing outline is simply
                            // clipped by the root `overflow_hidden` and wheel /
                            // trackpad events no-op — the main panel stopped
                            // scrolling (BugFunnel 2026-07-22). The shrink-drawer
                            // sidebar branch above already scrolls for the same
                            // reason; this is the matching viewport for the main
                            // panel. Harmless in the virtualized case: a
                            // `size_full` `gpui::list` never overflows this
                            // wrapper, so the scroll is a no-op there.
                            .overflow_y_scroll()
                            .flex_col()
                            .px(px(pad_x))
                            .py(px(pad_y))
                            .child(rendered)
                            .into_any_element()
                    }));
                }
            }
        }
    }

    for (is_right, el) in overlay_elements {
        let wrapper = div().absolute().top_0().h_full();
        let wrapper = if is_right {
            wrapper.right_0()
        } else {
            wrapper.left_0()
        };
        container = container.child(wrapper.child(el));
    }

    container
}

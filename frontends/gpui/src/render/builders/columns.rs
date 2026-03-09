use std::sync::Arc;

use super::prelude::*;
use holon_frontend::reactive_view_model::{ReactiveViewKind, ReactiveViewModel};

/// Wraps a rendered panel in the `flex_1 relative → absolute size_full`
/// layout chain that hands a descendant scrollable list a definite viewport.
/// Without this chain the panel has no height constraint, `ReactiveShell`'s
/// list collapses to zero, and wheel events no-op.
///
/// `build_inner` receives the bare absolute div and returns a finished
/// element — callers add id / overflow / padding / child at one site. The
/// closure returns `AnyElement` so it can wrap the div in `.id()` (which
/// produces `Stateful<Div>`) without the helper caring about the concrete
/// type.
fn panel_wrap(build_inner: impl FnOnce(Div) -> gpui::AnyElement) -> Div {
    let inner = div().absolute().top_0().left_0().size_full();
    div().flex_1().relative().child(build_inner(inner))
}

pub fn render(gap: &f32, children: &Vec<Arc<ReactiveViewModel>>, ctx: &GpuiRenderContext) -> Div {
    let items = children.clone();

    let has_drawers = items
        .iter()
        .any(|n| matches!(n.kind, ReactiveViewKind::Drawer { .. }));
    if !has_drawers {
        // `items_stretch` and `size_full` on the outer row make `flex_1`
        // children have something concrete to distribute.
        let mut container = div()
            .flex()
            .flex_row()
            .items_stretch()
            .size_full()
            .gap(px(*gap));
        for item in &items {
            let rendered = super::render(item, ctx);
            container =
                container.child(panel_wrap(|inner| inner.child(rendered).into_any_element()));
        }
        return container;
    }

    // `size_full` rather than `flex_1 w_full` — `flex_1` is inert unless
    // the parent is a flex container, which for reactive column roots
    // (e.g. the main app layout) isn't guaranteed. Matches the
    // non-drawers branch above. Without this, the columns row collapses
    // to intrinsic height (0 — all descendants are flex-allocated or
    // absolute-positioned so they contribute nothing to intrinsic size),
    // and every panel beneath inherits the zero height.
    let mut container = div().flex().flex_row().items_stretch().size_full();

    let first_drawer = items
        .iter()
        .position(|n| matches!(n.kind, ReactiveViewKind::Drawer { .. }));
    let last_drawer = items
        .iter()
        .rposition(|n| matches!(n.kind, ReactiveViewKind::Drawer { .. }));

    for (i, node) in items.iter().enumerate() {
        let is_first_drawer = Some(i) == first_drawer;
        let is_last_drawer = Some(i) == last_drawer && first_drawer != last_drawer;

        if let ReactiveViewKind::Drawer {
            ref block_id,
            ref child,
        } = node.kind
        {
            if is_first_drawer || is_last_drawer {
                let is_open = ctx.services.widget_state(block_id).open;

                let rendered = super::render(child, ctx);
                let inner = div()
                    .id(hashed_id(&block_id))
                    .h_full()
                    .overflow_y_scroll()
                    .bg(tc(ctx, |t| t.sidebar))
                    .w(px(ctx.style().sidebar_width))
                    .min_w(px(ctx.style().sidebar_width))
                    .px(px(ctx.style().sidebar_padding_x))
                    .py(px(ctx.style().sidebar_padding_y))
                    .text_sm()
                    .child(rendered);
                let inner = if is_first_drawer {
                    inner.border_r_1().border_color(tc(ctx, |t| t.border))
                } else {
                    inner.border_l_1().border_color(tc(ctx, |t| t.border))
                };

                let target_width = if is_open {
                    ctx.style().sidebar_width
                } else {
                    0.0
                };

                // Render at target width directly — see `drawer.rs` for
                // why the slide animation was removed. Restoring it
                // requires tracking previous open state so a no-op
                // transition doesn't replay from 0 on every render.
                container = container.child(
                    div()
                        .h_full()
                        .overflow_hidden()
                        .flex_shrink_0()
                        .w(px(target_width))
                        .child(inner),
                );
            } else {
                // Non-first/last drawer — render as normal panel
                let rendered = super::render(child, ctx);
                let id = hashed_id(&block_id);
                container = container.child(panel_wrap(|inner| {
                    inner
                        .id(id)
                        .overflow_y_scroll()
                        .p_2()
                        .child(rendered)
                        .into_any_element()
                }));
            }
        } else {
            let scroll_id = node
                .entity
                .get("id")
                .and_then(|v| v.as_string())
                .unwrap_or("panel");
            let rendered = super::render(node, ctx);
            let id = hashed_id(&scroll_id.to_string());
            let pad_x = ctx.style().content_padding_x;
            let pad_y = ctx.style().content_padding_y;
            // Absolute pattern gives definite height from items_stretch.
            // No overflow_y_scroll — children handle their own scrolling.
            container = container.child(panel_wrap(move |inner| {
                inner
                    .id(id)
                    .flex_col()
                    .px(px(pad_x))
                    .py(px(pad_y))
                    .child(rendered)
                    .into_any_element()
            }));
        }
    }

    container
}

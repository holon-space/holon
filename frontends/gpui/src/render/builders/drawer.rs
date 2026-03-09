use super::prelude::*;
use holon_frontend::reactive_view_model::{ReactiveViewKind, ReactiveViewModel};

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let ReactiveViewKind::Drawer { block_id, child } = &node.kind else {
        unreachable!()
    };

    let is_open = ctx.services.widget_state(block_id).open;
    let sidebar_width = ctx.style().sidebar_width;
    let target_width = if is_open { sidebar_width } else { 0.0 };

    let rendered = super::render(child, ctx);
    let inner = div()
        .id(hashed_id(&block_id))
        .h_full()
        .overflow_y_scroll()
        .bg(tc(ctx, |t| t.sidebar))
        .w(px(sidebar_width))
        .min_w(px(sidebar_width))
        .px(px(ctx.style().sidebar_padding_x))
        .py(px(ctx.style().sidebar_padding_y))
        .text_sm()
        .border_r_1()
        .border_color(tc(ctx, |t| t.border))
        .child(rendered);

    // Render at target width directly — no slide animation. The previous
    // `with_animation` wrapper started every render at `start_width=0`
    // and lerped to `target_width`, which left layout invariants
    // unable to distinguish "mid open animation" from "drawer never
    // rendered" (proptest on `Drawer(ReactiveList(0))`). Restoring the
    // animation requires tracking previous open state so a no-op
    // transition doesn't replay from 0 on every render.
    div()
        .h_full()
        .overflow_hidden()
        .flex_shrink_0()
        .w(px(target_width))
        .child(inner)
        .into_any_element()
}

use super::prelude::*;
use holon_frontend::reactive_view_model::ReactiveViewModel;
use holon_frontend::view_model::DrawerMode;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let block_id = node.prop_str("block_id").unwrap_or_else(|| "".to_string());
    let mode = DrawerMode::from_str(&node.prop_str("mode").unwrap_or_else(|| "shrink".to_string()));
    let width = node.prop_f64("width").unwrap_or(300.0) as f32;
    let child = node.children.first().expect("drawer requires a child");

    let is_open = ctx.services.widget_state(&block_id).open;

    let rendered = super::render(child, ctx);
    let inner = div()
        .id(hashed_id(&block_id))
        .h_full()
        .overflow_y_scroll()
        .bg(tc(ctx, |t| t.sidebar))
        .w(px(width))
        .min_w(px(width))
        .px(px(ctx.style().sidebar_padding_x))
        .py(px(ctx.style().sidebar_padding_y))
        .text_sm()
        .border_r_1()
        .border_color(tc(ctx, |t| t.border))
        .child(rendered);

    match mode {
        DrawerMode::Shrink => {
            // Shrink: takes layout space when open, collapses to 0 when closed.
            let target_width = if is_open { width } else { 0.0 };
            div()
                .h_full()
                .overflow_hidden()
                .flex_shrink_0()
                .w(px(target_width))
                .child(inner)
                .into_any_element()
        }
        DrawerMode::Overlay => {
            // Overlay: float above sibling content. The surrounding
            // `columns::render` is responsible for anchoring this panel
            // to the correct edge of the container (left or right) via
            // an absolute-positioned wrapper — we just return the panel
            // content (or an empty placeholder when closed). Previously
            // this wrapped the panel in a *zero-width relative outer*
            // placed at the end of the flex row; the absolute child then
            // anchored off-screen to the right of the container.
            if is_open {
                inner.into_any_element()
            } else {
                div().w(px(0.0)).h(px(0.0)).into_any_element()
            }
        }
    }
}

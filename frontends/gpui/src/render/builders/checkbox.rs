use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let checked = node.prop_bool("checked").unwrap_or(false);
    let icon_size = ctx.style().icon_size;
    if checked {
        div()
            .text_size(px(icon_size))
            .text_color(tc(ctx, |t| t.success))
            .child("\u{25C9}") // filled circle
    } else {
        div()
            .text_size(px(icon_size))
            .text_color(tc(ctx, |t| t.muted_foreground))
            .child("\u{25CB}") // open circle
    }
}

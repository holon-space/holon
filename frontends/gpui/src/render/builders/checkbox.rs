use super::prelude::*;

pub fn render(checked: &bool, ctx: &GpuiRenderContext) -> Div {
    let icon_size = ctx.style().icon_size;
    if *checked {
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

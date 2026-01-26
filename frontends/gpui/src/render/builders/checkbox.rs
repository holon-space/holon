use super::prelude::*;

pub fn render(checked: &bool, ctx: &GpuiRenderContext) -> Div {
    if *checked {
        div()
            .text_size(px(16.0))
            .text_color(tc(ctx, |t| t.success))
            .child("\u{25C9}") // ◉ filled circle
    } else {
        div()
            .text_size(px(16.0))
            .text_color(tc(ctx, |t| t.muted_foreground))
            .child("\u{25CB}") // ○ open circle
    }
}

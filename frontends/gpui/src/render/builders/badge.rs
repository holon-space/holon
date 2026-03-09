use super::prelude::*;

pub fn render(label: &String, ctx: &GpuiRenderContext) -> Div {
    div()
        .px(px(8.0))
        .py(px(2.0))
        .rounded(px(10.0))
        .bg(tc(ctx, |t| t.secondary))
        .text_size(px(11.0))
        .text_color(tc(ctx, |t| t.muted_foreground))
        .child(label.clone())
}

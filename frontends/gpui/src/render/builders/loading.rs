use super::prelude::*;

pub fn render(ctx: &GpuiRenderContext) -> Div {
    div()
        .p_2()
        .text_color(tc(ctx, |t| t.muted_foreground))
        .text_sm()
        .child("Loading…")
}

use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(_node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    div()
        .p_2()
        .text_color(tc(ctx, |t| t.muted_foreground))
        .text_sm()
        .child("Loading…")
}

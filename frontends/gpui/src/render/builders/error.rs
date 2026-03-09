use super::prelude::*;

pub fn render(message: &String, ctx: &GpuiRenderContext) -> Div {
    div()
        .p_2()
        .rounded(px(4.0))
        .bg(tc(ctx, |t| t.secondary))
        .text_color(tc(ctx, |t| t.danger))
        .text_sm()
        .child(message.clone())
}

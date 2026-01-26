use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let message = node.prop_str("message").unwrap_or_else(|| "".to_string());
    div()
        .p_2()
        .rounded(px(4.0))
        .bg(tc(ctx, |t| t.secondary))
        .text_color(tc(ctx, |t| t.danger))
        .text_sm()
        .child(message)
}

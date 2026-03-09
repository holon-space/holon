use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let label = node.prop_str("label").unwrap_or_else(|| "".to_string());
    div()
        .px(px(8.0))
        .py(px(2.0))
        .text_size(px(11.0))
        .text_color(tc(ctx, |t| t.accent))
        .child(label.to_string())
}

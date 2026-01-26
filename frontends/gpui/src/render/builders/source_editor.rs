use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let language = node
        .prop_str("language")
        .unwrap_or_else(|| "text".to_string());
    let content = node.prop_str("content").unwrap_or_default();
    div()
        .size_full()
        .p_2()
        .bg(tc(ctx, |t| t.secondary))
        .text_xs()
        .flex_col()
        .child(
            div()
                .text_color(tc(ctx, |t| t.muted_foreground))
                .text_xs()
                .child(language),
        )
        .child(div().child(content))
}

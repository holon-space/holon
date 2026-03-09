use super::prelude::*;

pub fn render(language: &String, content: &String, ctx: &GpuiRenderContext) -> Div {
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
                .child(language.clone()),
        )
        .child(div().child(content.clone()))
}

use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let title = node.prop_str("title").unwrap_or_else(|| "".to_string());
    let children = &node.children;
    let mut container = div().size_full().flex_1().flex_col().gap(px(8.0));

    container = container.child(
        div()
            .w_full()
            .pb(px(8.0))
            .child(
                div()
                    .text_size(px(28.0))
                    .font_weight(gpui::FontWeight::BOLD)
                    .text_color(tc(ctx, |t| t.foreground))
                    .child(title.clone()),
            ),
    );

    for child in render_children(children, ctx) {
        container = container.child(child);
    }

    container
}

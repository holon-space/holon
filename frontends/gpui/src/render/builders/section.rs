use super::prelude::*;
use holon_frontend::view_model::LazyChildren;

pub fn render(title: &String, children: &LazyChildren, ctx: &GpuiRenderContext) -> Div {
    let mut container = div().w_full().flex_col().mt(px(16.0)).mb(px(4.0));

    container = container.child(
        div()
            .w_full()
            .pb(px(8.0))
            .mb(px(6.0))
            .border_b_1()
            .border_color(tc(ctx, |t| t.border))
            .child(
                div()
                    .text_size(px(11.0))
                    .font_weight(gpui::FontWeight::SEMIBOLD)
                    .text_color(tc(ctx, |t| t.muted_foreground))
                    .child(title.to_uppercase()),
            ),
    );

    for child in render_children(children, ctx) {
        container = container.child(child);
    }

    container
}

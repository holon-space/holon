use super::prelude::*;
use holon_frontend::view_model::LazyChildren;

pub fn render(children: &LazyChildren, ctx: &GpuiRenderContext) -> Div {
    let rendered = render_children(children, ctx);
    if rendered.is_empty() {
        return div()
            .text_size(px(13.0))
            .text_color(tc(ctx, |t| t.muted_foreground))
            .child("[no result]");
    }
    let mut container = div().flex_col().gap(px(1.0));
    for child in rendered {
        container = container.child(child);
    }
    container
}

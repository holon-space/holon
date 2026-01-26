use super::prelude::*;
use holon_frontend::view_model::LazyChildren;

pub fn render(gap: &f32, children: &LazyChildren, ctx: &GpuiRenderContext) -> Div {
    let effective_gap = gap.max(4.0);
    let mut container = div().flex_col().gap(px(effective_gap));
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}

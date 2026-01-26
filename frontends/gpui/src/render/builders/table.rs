use super::prelude::*;
use holon_frontend::view_model::LazyChildren;

pub fn render(children: &LazyChildren, ctx: &GpuiRenderContext) -> Div {
    let mut container = div().flex_col().gap_0p5();
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}

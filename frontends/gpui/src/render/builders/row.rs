use super::prelude::*;
use holon_frontend::view_model::LazyChildren;

pub fn render(gap: &f32, children: &LazyChildren, ctx: &GpuiRenderContext) -> Div {
    let mut container = div().flex().flex_row().gap(px(*gap)).items_center();
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}

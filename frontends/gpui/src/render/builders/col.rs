use std::sync::Arc;

use super::prelude::*;

pub fn render(gap: &f32, children: &Vec<Arc<holon_frontend::reactive_view_model::ReactiveViewModel>>, ctx: &GpuiRenderContext) -> Div {
    let mut container = div().flex().flex_col();
    if *gap > 0.0 {
        container = container.gap(px(*gap));
    }
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}

use std::sync::Arc;

use super::prelude::*;

pub fn render(gap: &f32, children: &Vec<Arc<holon_frontend::reactive_view_model::ReactiveViewModel>>, ctx: &GpuiRenderContext) -> Div {
    let mut container = div().w_full().flex().flex_row().gap(px(*gap)).items_center();
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}

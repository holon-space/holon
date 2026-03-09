use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let gap = node.prop_f64("gap").unwrap_or(8.0) as f32;
    let children = &node.children;

    let mut container = div().w_full().flex().flex_row().gap(px(gap)).items_center();
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}

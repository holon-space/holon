use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let gap = node.prop_f64("gap").unwrap_or(0.0) as f32;
    let children = &node.children;

    let mut container = div().flex().flex_col();
    if gap > 0.0 {
        container = container.gap(px(gap));
    }
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}

use holon_frontend::ReactiveViewModel;

use super::prelude::*;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> Div {
    let gap = node.prop_f64("gap").unwrap_or(8.0) as f32;
    let align = node
        .prop_str("align")
        .unwrap_or_else(|| "center".to_string());
    let children = &node.children;

    let mut container = div().w_full().flex().flex_row().gap(px(gap));
    container = match align.as_str() {
        "start" => container.items_start(),
        "end" => container.items_end(),
        _ => container.items_center(),
    };
    for child in render_children(children, ctx) {
        container = container.child(child);
    }
    container
}

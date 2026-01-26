use super::prelude::*;
use holon_frontend::ReactiveViewModel;

pub fn render(node: &ReactiveViewModel, _ctx: &GpuiRenderContext) -> Div {
    let data = node.entity();
    let mut row_div = div()
        .flex()
        .flex_row()
        .gap(px(12.0))
        .min_h(px(28.0))
        .items_center();
    let mut keys: Vec<&String> = data.keys().collect();
    keys.sort();
    for key in keys {
        let val = data.get(key).map(|v| v.to_display_string()).unwrap_or_default();
        row_div = row_div.child(
            div()
                .w(px(140.0))
                .text_size(px(13.0))
                .line_height(px(20.0))
                .child(val),
        );
    }
    row_div
}

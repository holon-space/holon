use super::prelude::*;
use holon_frontend::render_interpreter::shared_tree_build;

pub fn build(ba: BA<'_>) -> Div {
    let items = shared_tree_build(&ba);

    if items.is_empty() {
        return div().child("[outline: no item_template]");
    }

    let mut container = div().flex_col().gap_0p5();
    for (widget, depth) in items {
        let indent = (depth as f32) * 16.0;
        container = container.child(
            div()
                .flex()
                .flex_row()
                .pl(px(indent))
                .child(widget),
        );
    }
    container
}

use holon_frontend::ReactiveViewModel;
use holon_frontend::user_driver::drag_drop_operation;

use super::prelude::*;
use crate::render::drag::DraggedBlock;
use crate::render::drag::make_drag_preview;

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let child = node.children.first().expect("draggable requires a child");
    let child_el = super::render(child, ctx);

    if drag_drop_operation(&node.operations).is_none() {
        return child_el;
    }
    let block_id = node
        .row_id()
        .expect("draggable binds its operation from a bound row, so it names one");

    let label = node
        .entity()
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .chars()
        .take(40)
        .collect::<String>();
    let bg_color = tc(ctx, |t| t.secondary);
    let text_color = tc(ctx, |t| t.foreground);

    let el_id = format!("drag-{block_id}");
    let payload = DraggedBlock { block_id };

    div()
        .child(
            div()
                .id(hashed_id(&el_id))
                .cursor_move()
                .child(child_el)
                .on_drag(payload, move |_info, position, _window, cx| {
                    cx.new(|_| make_drag_preview(label.clone(), position, bg_color, text_color))
                }),
        )
        .into_any_element()
}

use super::prelude::*;
use holon_frontend::ReactiveViewModel;

use crate::render::drag::{DraggedBlock, make_drag_preview};

pub fn render(node: &ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::reactive_view_model::ReactiveViewKind;
    let ReactiveViewKind::Draggable { child } = &node.kind else {
        unreachable!()
    };

    let child_el = super::render(child, ctx);

    let Some(block_id) = node.row_id() else {
        return child_el;
    };

    let label = node
        .entity
        .get("content")
        .and_then(|v| v.as_string())
        .unwrap_or("")
        .chars()
        .take(40)
        .collect::<String>();
    let bg_color = tc(ctx, |t| t.secondary);
    let text_color = tc(ctx, |t| t.foreground);

    let el_id = format!("drag-{block_id}");
    let payload = DraggedBlock {
        block_id,
        entity: node.entity.clone(),
        operations: node.operations.clone(),
    };

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

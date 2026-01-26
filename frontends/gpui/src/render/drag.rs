use std::collections::HashMap;
use std::sync::Arc;

use gpui::prelude::*;
use gpui::{div, px, Context, Hsla, Pixels, Point, Window};
use holon_api::render_types::OperationWiring;
use holon_api::Value;

/// Payload carried during a block drag operation.
/// Contains all data needed by drop_zone to dispatch a move_block operation.
#[derive(Clone)]
#[allow(dead_code)]
pub struct DraggedBlock {
    pub block_id: String,
    pub entity: Arc<HashMap<String, Value>>,
    pub operations: Vec<OperationWiring>,
}

/// Visual preview shown while dragging a block.
pub(crate) struct DragPreview {
    label: String,
    position: Point<Pixels>,
    bg: Hsla,
    text_color: Hsla,
}

pub(crate) fn make_drag_preview(
    label: String,
    position: Point<Pixels>,
    bg: Hsla,
    text_color: Hsla,
) -> DragPreview {
    DragPreview {
        label,
        position,
        bg,
        text_color,
    }
}

impl Render for DragPreview {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<'_, Self>) -> impl IntoElement {
        let size = gpui::size(px(200.0), px(32.0));

        div()
            .pl(self.position.x - size.width / 2.0)
            .pt(self.position.y - size.height / 2.0)
            .child(
                div()
                    .flex()
                    .items_center()
                    .w(size.width)
                    .h(size.height)
                    .px_2()
                    .rounded(px(4.0))
                    .bg(self.bg)
                    .text_color(self.text_color)
                    .text_xs()
                    .shadow_md()
                    .overflow_hidden()
                    .child(if self.label.is_empty() {
                        "Block".to_string()
                    } else {
                        self.label.clone()
                    }),
            )
    }
}

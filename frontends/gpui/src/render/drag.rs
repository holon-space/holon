use gpui::Context;
use gpui::Hsla;
use gpui::Pixels;
use gpui::Point;
use gpui::Window;
use gpui::div;
use gpui::prelude::*;
use gpui::px;

/// Payload carried during a block drag: the dragged row, which a drop zone
/// moves with its own bound operation. Only a `draggable` that binds its drag
/// operation starts one.
#[derive(Clone)]
pub struct DraggedBlock {
    pub block_id: holon_api::EntityUri,
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
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
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

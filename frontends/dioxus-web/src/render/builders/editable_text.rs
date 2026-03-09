use holon_frontend::view_model::ViewKind;

use super::prelude::*;
use crate::editor::EditorCell;
use crate::render::EntityContext;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::EditableText { content, .. } = &node.kind else {
        return rsx! {};
    };
    let content = content.clone();
    // GPUI keys each editor by the node's own row id (virtual rows —
    // `block:__virtual:*` empty-collection placeholders — have a row id
    // that differs from the enclosing live_block). The worker's focus
    // authority speaks row ids, so the DOM element must be keyed the
    // same way or `worker_focus::apply` can never find it.
    let row_id = node.row_id();
    rsx! { EditableTextNode { content, row_id } }
}

#[component]
fn EditableTextNode(content: String, row_id: Option<String>) -> Element {
    let entity_id = row_id
        .or_else(|| try_consume_context::<EntityContext>().map(|ctx| ctx.0))
        .unwrap_or_else(|| {
            tracing::error!(
                "[render] editable_text without row_id or EntityContext — keystrokes on this cell \
                 will be dropped"
            );
            String::new()
        });
    rsx! { EditorCell { entity_id, content } }
}

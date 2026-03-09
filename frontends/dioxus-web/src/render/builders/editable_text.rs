use super::prelude::*;
use crate::editor::EditorCell;
use crate::render::EntityContext;

pub fn render(
    content: &String,
    _field: &String,
    _ctx: &DioxusRenderContext,
) -> Element {
    let content = content.clone();
    rsx! { EditableTextNode { content } }
}

#[component]
fn EditableTextNode(content: String) -> Element {
    let entity_id = try_consume_context::<EntityContext>()
        .map(|ctx| ctx.0)
        .unwrap_or_else(|| {
            tracing::error!(
                "[render] editable_text rendered without EntityContext — \
                 parent must be a live_block"
            );
            String::new()
        });
    rsx! { EditorCell { entity_id, content } }
}

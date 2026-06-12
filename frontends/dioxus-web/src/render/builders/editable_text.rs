use super::prelude::*;
use crate::editor::EditorCell;
use crate::render::EntityContext;
use holon_frontend::view_model::ViewKind;

pub fn render(node: &ViewModel, _: &DioxusRenderContext) -> Element {
    let ViewKind::EditableText { content, .. } = &node.kind else {
        return rsx! {};
    };
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

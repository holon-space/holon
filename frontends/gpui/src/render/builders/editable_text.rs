use super::prelude::*;

pub fn render(node: &holon_frontend::ViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::view_model::NodeKind;
    let NodeKind::EditableText { content, field } = &node.kind else {
        unreachable!()
    };

    let row_id = super::operation_helpers::row_id_from_node(node);

    // Look up pre-created EditorView entity from the registry.
    if let Some(ref id) = row_id {
        let el_id = format!("editable-text-{id}-{field}");
        if let Some(entity) = ctx.bounds_registry.get_editor_view(&el_id) {
            return entity.into_any_element();
        }
    }

    // Fallback: render as static text (no entity ID or not yet in registry).
    let text_color = tc(ctx, |t| t.foreground);
    let display_text = if content.is_empty() {
        "(empty)".to_string()
    } else {
        content.clone()
    };

    div()
        .w_full()
        .min_h(px(26.0))
        .py(px(1.0))
        .text_color(text_color)
        .text_size(px(15.0))
        .line_height(px(22.0))
        .child(display_text)
        .into_any_element()
}

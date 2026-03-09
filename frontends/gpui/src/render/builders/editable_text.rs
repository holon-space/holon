use super::prelude::*;
use crate::views::EditorView;

pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    use holon_frontend::reactive_view_model::ReactiveViewKind;
    let ReactiveViewKind::EditableText { content, field } = &node.kind else {
        unreachable!()
    };

    let Some(row_id) = node.row_id() else {
        return static_fallback(content, ctx);
    };

    let el_id = format!("editable-text-{row_id}-{field}");

    // Look up existing EditorView from the global registry.
    if let Some(entity) = ctx.focus.editor_views.get(&el_id) {
        let inner = entity.into_any_element();
        return crate::geometry::tracked(el_id, inner, &ctx.bounds_registry, "editable_text", Some(&row_id), true).into_any_element();
    }

    // Create a new EditorView and register it globally.
    let has_content = !content.is_empty();
    let content = content.clone();
    let field = field.clone();
    let operations = node.operations.clone();
    let triggers = node.triggers.clone();
    let services = ctx.services.clone();
    let focus = ctx.focus.clone();

    ctx.with_gpui(|window, cx| {
        let svc = services.clone();
        let entity = cx.new(|cx| {
            EditorView::new(
                el_id.clone(),
                content,
                field,
                row_id.clone(),
                operations,
                triggers,
                svc,
                window,
                cx,
            )
        });
        let input = entity.read(cx).input_entity().clone();
        let row_id_ref = row_id.clone();
        focus.editor_inputs.register(row_id, input);
        focus.editor_views.register(el_id.clone(), entity.clone());
        let inner = entity.into_any_element();
        crate::geometry::tracked(el_id, inner, &ctx.bounds_registry, "editable_text", Some(&row_id_ref), has_content).into_any_element()
    })
}

fn static_fallback(content: &str, ctx: &GpuiRenderContext) -> AnyElement {
    let text_color = tc(ctx, |t| t.foreground);
    let display_text = if content.is_empty() {
        "(empty)".to_string()
    } else {
        content.to_string()
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

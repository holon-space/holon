use super::prelude::*;
use crate::views::EditorView;

pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let content = node.prop_str("content").unwrap_or_else(|| "".to_string());
    let field = node.prop_str("field").unwrap_or_else(|| "content".to_string());

    let Some(row_id) = node.row_id() else {
        return static_fallback(&content, ctx);
    };

    let el_id = format!("editable-text-{row_id}-{field}");
    let has_content = !content.is_empty();

    // The EditorView entity is parent-owned via `LocalEntityScope`'s
    // `EntityCache`: each `RenderEntityView` / `ReactiveShell` keeps its
    // own cache, so an editor lives exactly as long as the row that owns
    // it. When the row is removed (collection driver `RemoveAt`) the
    // cache drops with the parent and the editor's `Task<()>`s
    // (`_data_subscription`, `_cursor_subscription`) cancel naturally.
    //
    // Render never touches `InputState::set_value` for sync — see
    // `EditorView::new`'s `_data_subscription` for backend → InputState
    // propagation (gated on focus to avoid clobbering live typing).
    let operations = node.operations.clone();
    let triggers = node.triggers.clone();
    let services = ctx.services.clone();
    let nav = ctx.nav.clone();
    let data_handle = Some(node.data.clone());
    let el_id_for_create = el_id.clone();
    let row_id_for_create = row_id.clone();
    let content_for_create = content.clone();
    let field_for_create = field.clone();

    let key = crate::entity_view_registry::CacheKey::Ephemeral(el_id.clone());
    let any = ctx.local.get_or_create(key, || {
        ctx.with_gpui(|window, cx| {
            cx.new(|cx| {
                EditorView::new(
                    el_id_for_create,
                    content_for_create,
                    field_for_create,
                    row_id_for_create,
                    operations,
                    triggers,
                    services,
                    nav,
                    data_handle,
                    window,
                    cx,
                )
            })
            .into_any()
        })
    });
    let entity: gpui::Entity<EditorView> = any.downcast().expect("editable_text cache type mismatch");

    // Snapshot the live `InputState` value so PBT invariants can detect
    // UI staleness (e.g. `editable_text` failing to follow SQL
    // `block.content` after `split_block` / `join_block`).
    let displayed_text: String = ctx.with_gpui(|_window, cx| {
        entity.read(cx).input_entity().read(cx).value().to_string()
    });
    let inner = entity.into_any_element();

    // Grey placeholder hint for empty editors — helps users discover that
    // typing into an empty block creates content. Rendered as an
    // absolutely-positioned BEHIND the real Input so it doesn't intercept
    // clicks or typing (GPUI hit-tests children in reverse paint order).
    let element = if !has_content {
        div()
            .relative()
            .child(
                div()
                    .absolute()
                    .top(px(4.0))
                    .left(px(0.0))
                    .text_color(gpui::Hsla {
                        h: 0.0,
                        s: 0.0,
                        l: 0.5,
                        a: 0.5,
                    })
                    .text_size(px(15.0))
                    .line_height(px(22.0))
                    .child("Type here to add a new block"),
            )
            .child(inner)
            .into_any_element()
    } else {
        inner
    };

    crate::geometry::tracked(
        el_id,
        element,
        &ctx.bounds_registry,
        "editable_text",
        Some(&row_id),
        has_content,
        Some(displayed_text),
    )
    .into_any_element()
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

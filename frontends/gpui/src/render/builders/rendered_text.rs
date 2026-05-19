use super::prelude::*;
use holon_api::EntityUri;

/// Read-only sibling of `editable_text`. Renders the block's content as
/// static text. A click dispatches `navigation.editor_focus` with
/// `cursor_offset = 0`, which flips the `is_focused` variant in
/// `block_profile.yaml` and swaps in `editable_text` on the next render.
/// The freshly-mounted editor's cursor subscription picks up the
/// `current_editor_focus` row via the cached `Mutable` signal value and
/// grabs window focus on its own.
pub fn render(
    node: &holon_frontend::ReactiveViewModel,
    ctx: &GpuiRenderContext,
) -> AnyElement {
    let content = node.prop_str("content").unwrap_or_default();
    let field = node.prop_str("field").unwrap_or_else(|| "content".to_string());

    let Some(row_id) = node.row_id() else {
        return static_inner(&content, ctx).into_any_element();
    };

    let el_id = format!("rendered-text-{row_id}-{field}");
    let has_content = !content.is_empty();
    let displayed = content.clone();
    let row_id_for_uri = row_id.clone();
    let services = ctx.services.clone();

    // Place the cursor at the end of the existing text rather than at
    // position 0 — clicking a read-only block almost always means "I
    // want to add to this," not "I want to insert at the very start."
    // TODO: for pixel-accurate cursor placement at the clicked glyph,
    // a custom GPUI element is needed: shape the text via
    // `window.text_system().shape_text(...)`, retain the resulting
    // `WrappedLine`s in the element's prepaint state, and in
    // `on_mouse_down` translate the local click position to a byte
    // offset via `WrappedLine::closest_index_for_position`. The plain
    // `div().child(string)` text element used here doesn't surface
    // that layout to event handlers.
    let initial_cursor_offset = content.len() as i64;
    let inner = div()
        .id(hashed_id(&el_id))
        .cursor_pointer()
        .child(static_inner(&content, ctx))
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
            let block_uri = EntityUri::from_raw(&row_id_for_uri);
            // Backend `navigation.editor_focus` expects the BARE id (no
            // `block:` scheme) because `editor_cursor.block_id` joins
            // `block.id` in `current_editor_focus`, and `block.id` is
            // stored unprefixed. Passing the full URI silently misses the
            // join and the freshly-mounted editor never grabs focus.
            let bare_id = block_uri.id().to_string();
            services.set_focus(Some(block_uri));
            let mut params = std::collections::HashMap::new();
            params.insert("region".into(), holon_api::Value::String("main".into()));
            params.insert("block_id".into(), holon_api::Value::String(bare_id));
            params.insert(
                "cursor_offset".into(),
                holon_api::Value::Integer(initial_cursor_offset),
            );
            services.dispatch_intent(holon_frontend::OperationIntent::new(
                "navigation".into(),
                "editor_focus".into(),
                params,
            ));
        })
        .into_any_element();

    crate::geometry::tracked(
        el_id,
        inner,
        &ctx.bounds_registry,
        "rendered_text",
        Some(&row_id),
        has_content,
        Some(displayed),
    )
    .into_any_element()
}

/// Static text element matching `editable_text`'s visual metrics so the
/// transition from read-only → editable doesn't cause a perceptible jump
/// when focus changes.
///
/// `Input::render` (gpui-component) — even with `appearance(false)` —
/// always applies `input_px(self.size)`, `input_py(self.size)`,
/// `input_text_size(self.size)`, and `line_height(Rems(1.25))`. For the
/// default `Size::Medium` that is `px(12)` horizontal, `px(8)` vertical,
/// `text_sm`, and `line_height` 1.25rem. We mirror those exactly here so
/// the swap from `rendered_text` → `editable_text` doesn't shift x/y or
/// resize the glyphs.
fn static_inner(content: &str, _: &GpuiRenderContext) -> Div {
    let display: String = if content.is_empty() {
        // Mirror `editable_text`'s empty-placeholder hint so unfocused
        // empty blocks still read as clickable instead of "nothing here".
        "Type here to add a new block".to_string()
    } else {
        content.to_string()
    };
    let mut el = div()
        .w_full()
        .px(px(12.0))
        .py(px(8.0))
        .text_sm()
        .line_height(gpui::Rems(1.25))
        .child(display);
    if content.is_empty() {
        el = el.text_color(gpui::Hsla {
            h: 0.0,
            s: 0.0,
            l: 0.5,
            a: 0.5,
        });
    }
    el
}

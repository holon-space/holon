use holon_api::EntityUri;

use super::prelude::*;

/// Read-only sibling of `editable_text`. Renders the block's content as
/// static text. A click calls `services.set_focus` (ADR 0010: focus is pure
/// in-memory state), which flips the `is_focused` variant in
/// `block_profile.yaml` and swaps in `editable_text` on the next render. The
/// freshly-mounted editor grabs window focus off the `focused_block` signal.
pub fn render(node: &holon_frontend::ReactiveViewModel, ctx: &GpuiRenderContext) -> AnyElement {
    let content = node.prop_str("content").unwrap_or_default();
    let field = node
        .prop_str("field")
        .unwrap_or_else(|| "content".to_string());

    let Some(row_id) = node.row_id() else {
        return static_inner(&content, ctx).into_any_element();
    };

    let el_id = format!("rendered-text-{row_id}-{field}");
    let has_content = !content.is_empty();
    let services = ctx.services.clone();

    // TODO: for pixel-accurate cursor placement at the clicked glyph,
    // a custom GPUI element is needed: shape the text via
    // `window.text_system().shape_text(...)`, retain the resulting
    // `WrappedLine`s in the element's prepaint state, and in
    // `on_mouse_down` translate the local click position to a byte
    // offset via `WrappedLine::closest_index_for_position`. The plain
    // `div().child(string)` text element used here doesn't surface
    // that layout to event handlers.
    // ALLOW(entity_uri_from_raw): render-spec rendered_text node row_id (boundary,
    // parsed once for the click target)
    let block_uri = EntityUri::from_raw(&row_id);
    let inner = click_to_focus(
        &el_id,
        static_inner(&content, ctx).into_any_element(),
        block_uri,
        services,
    )
    .into_any_element();

    crate::geometry::tracked(
        el_id,
        inner,
        &ctx.bounds_registry,
        "rendered_text",
        Some(&row_id),
        has_content,
        Some(std::sync::Arc::from(content)),
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
        "Type here".to_string()
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

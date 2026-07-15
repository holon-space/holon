pub use gpui::div;
pub use gpui::prelude::*;
pub use gpui::px;
pub use gpui::AnyElement;
pub use gpui::Div;
pub use gpui::ElementId;
pub use gpui::Hsla;

pub(crate) use super::render_children;
pub(crate) use super::GpuiRenderContext;

/// Shorthand: get a theme color from gpui-component's global ThemeColor.
pub(crate) fn tc(
    ctx: &GpuiRenderContext,
    pick: impl FnOnce(&gpui_component::theme::ThemeColor) -> gpui::Hsla,
) -> Hsla {
    ctx.with_gpui(|_window, cx| {
        use gpui_component::theme::ActiveTheme;
        pick(&cx.theme().colors)
    })
}

/// Hash a string element ID to `ElementId::Integer` to avoid `Arc<[ElementId]>`
/// path bloat.
///
/// GPUI stores element identity as `Arc<[ElementId]>` path slices cloned at
/// every `request_layout`. `ElementId::Name(SharedString)` is 32 bytes per
/// entry; `ElementId::Integer(u64)` is 16 bytes — halving the per-path cost.
/// With deep element trees (hundreds of ID'd elements) this saves hundreds of
/// MB.
pub(crate) fn hashed_id(s: &str) -> ElementId {
    use std::hash::Hash;
    use std::hash::Hasher;
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    ElementId::Integer(hasher.finish())
}

/// Wrap `child` in a left-click-to-focus `div`. Clicking sets the in-memory
/// focus authority directly (ADR 0010: editor focus is pure in-memory UI
/// state) — the focused block's editor mounts and grabs window focus off the
/// `focused_block` signal, with no `editor_cursor` write or CDC round-trip.
///
/// Shared by the read-only render paths (`rendered_text` builder,
/// `render_entity_view`); each caller adds its own outer wrapping (e.g. bounds
/// tracking). The caret defaults to end-of-text on mount ("add to this block").
pub(crate) fn click_to_focus(
    el_id: &str,
    child: AnyElement,
    block: holon_api::EntityUri,
    services: std::sync::Arc<dyn holon_frontend::reactive::BuilderServices>,
) -> gpui::Stateful<Div> {
    div()
        .id(hashed_id(el_id))
        .cursor_pointer()
        .child(child)
        .on_mouse_down(gpui::MouseButton::Left, move |_, _, _| {
            services.set_focus(Some(block.clone()));
        })
}

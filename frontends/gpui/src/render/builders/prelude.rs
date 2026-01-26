pub use gpui::prelude::*;
pub use gpui::{div, px, AnyElement, Div, ElementId, Hsla};

pub(crate) use super::style::LayoutStyle;
pub(crate) use super::{render_children, GpuiRenderContext};

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

/// Hash a string element ID to `ElementId::Integer` to avoid `Arc<[ElementId]>` path bloat.
///
/// GPUI stores element identity as `Arc<[ElementId]>` path slices cloned at every
/// `request_layout`. `ElementId::Name(SharedString)` is 32 bytes per entry;
/// `ElementId::Integer(u64)` is 16 bytes — halving the per-path cost.
/// With deep element trees (hundreds of ID'd elements) this saves hundreds of MB.
pub(crate) fn hashed_id(s: &str) -> ElementId {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    ElementId::Integer(hasher.finish())
}

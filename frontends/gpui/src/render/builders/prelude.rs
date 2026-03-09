pub use gpui::prelude::*;
pub use gpui::{div, px, AnyElement, Div, ElementId, Hsla};

pub(crate) use super::{render_children, GpuiRenderContext};

/// Shorthand: get a theme color from the cached gpui-component ThemeColor snapshot.
pub(crate) fn tc(
    ctx: &GpuiRenderContext,
    pick: impl FnOnce(&gpui_component::theme::ThemeColor) -> gpui::Hsla,
) -> Hsla {
    let theme = ctx.bounds_registry.theme();
    pick(&theme)
}

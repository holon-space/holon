pub use gpui::prelude::*;
pub use gpui::{div, px, Div, Rgba};

pub(crate) use super::BA;

/// Shorthand: get a theme color as gpui::Rgba from the BoundsRegistry on the render context.
pub(crate) fn tc(
    ba: &BA<'_>,
    pick: impl FnOnce(&holon_frontend::theme::ThemeColors) -> holon_frontend::theme::Rgba8,
) -> Rgba {
    ba.ctx.ext.gpui_color(pick(ba.ctx.ext.theme()))
}

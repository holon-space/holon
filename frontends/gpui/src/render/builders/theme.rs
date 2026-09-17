//! The one place a theme token becomes a pixel in this frontend.
//!
//! `theme_token_color` is exhaustive over [`ThemeToken`], so a token added to
//! the vocabulary fails to compile here rather than reaching a catch-all. Each
//! token maps to the `gpui_component` slot `apply_holon_theme`
//! (`frontends/gpui/src/lib.rs`) fills from holon's own theme, which is what
//! makes a layout colour follow the active theme in both modes.
//!
//! `theme_token_from_prop` is the paint-side entry: it takes a name that has
//! already been through [`holon_frontend::theme_arg`] and reports one that has
//! not, because a paint pass has no error widget to render.

use holon_api::theme_token::ThemeToken;

use super::prelude::*;

/// Resolve `token` against the ACTIVE theme.
pub(crate) fn theme_token_color(ctx: &GpuiRenderContext, token: ThemeToken) -> Hsla {
    tc(ctx, |t| match token {
        ThemeToken::Accent => t.accent,
        // `error` on purpose, not `danger`: a layout says what it MEANS, and
        // the theme decides which slot carries it.
        ThemeToken::Error => t.danger,
        ThemeToken::Foreground => t.foreground,
        ThemeToken::Info => t.info,
        ThemeToken::Muted | ThemeToken::Secondary => t.muted_foreground,
        ThemeToken::Primary => t.primary,
        ThemeToken::Success => t.success,
        ThemeToken::Warning => t.warning,
    })
}

/// The token a colour prop names, disclosing one that reached paint
/// unvalidated.
///
/// A paint pass has no error widget, so the disclosure here is the log and the
/// body foreground. The conditions that cause it: a colour the layout doc
/// declared and the parse gate would have refused, or a row value the theme has
/// no slot for, as [`holon_frontend::theme_arg`] reports it.
pub(crate) fn theme_token_from_prop(name: &str) -> ThemeToken {
    match holon_frontend::theme_arg::resolve_colour_arg(name) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!("{e}; painting the body foreground instead");
            ThemeToken::Foreground
        }
    }
}

/// The colour a `color` prop names, or `None` when the prop is absent.
///
/// An EMPTY value means "unset", not a colour: `icon` defaults its colour
/// parameter to `""` and a `card` without an accent is legal.
pub(crate) fn optional_colour_prop(ctx: &GpuiRenderContext, name: Option<&str>) -> Option<Hsla> {
    let name = name?;
    if name.is_empty() {
        return None;
    }
    Some(theme_token_color(ctx, theme_token_from_prop(name)))
}

/// The packed `0xRRGGBBAA` form the tint blenders operate on (`card`, `board`).
///
/// One packing helper rather than the three copies that each kept their own
/// rounding and their own hardcoded default.
pub(crate) fn packed(colour: Hsla) -> u32 {
    u32::from_be_bytes(crate::geometry::hsla_to_rgba8(colour))
}

//! The one place a theme token becomes a pixel in this frontend.
//!
//! Before this, five sites in the GPUI frontend each turned a colour name into
//! a pixel with its own string match and its own catch-all: `text` knew
//! `muted`/`warning`/`success` and painted `foreground` for anything else,
//! `icon` knew a different set and painted `muted_foreground`, and `card` /
//! `board` accepted hex only and painted grey for a name. So the shipped
//! `card(accent: "primary")` and `icon(color: "primary")` in
//! `assets/default/types/block_profile.yaml` painted something the author never
//! asked for, and nothing said so.
//!
//! The match below is exhaustive over [`ThemeToken`], so a token added to the
//! vocabulary fails to compile here instead of falling into a catch-all. Each
//! token maps to the `gpui_component` slot `apply_holon_theme`
//! (`frontends/gpui/src/lib.rs`) fills from holon's own theme, which is what
//! makes a layout colour follow the active theme in both modes.

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

/// Parse a colour prop, disclosing a name that reached paint unvalidated.
///
/// A colour name is refused when the layout doc is loaded
/// (`holon_api::render_dsl`) and again when a builder reads it
/// (`shadow_builders`), so reaching here with an unknown name means an
/// expression was built in Rust and never went through either boundary. That is
/// a bug, and it is reported as one rather than absorbed: the alternative is
/// what this whole module replaces, where four sites quietly painted a colour
/// nobody asked for.
pub(crate) fn theme_token_from_prop(name: &str) -> ThemeToken {
    match ThemeToken::parse(name) {
        Ok(token) => token,
        Err(e) => {
            tracing::error!(
                "{e}; painting the body foreground instead. A colour name should have been \
                 refused when the expression was built (holon_api::theme_token)."
            );
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

//! The one place a theme token becomes a pixel in this frontend.
//!
//! waterui has no live theme wired: nothing here calls an equivalent of
//! `apply_holon_theme`, so a token cannot follow the user's choice the way the
//! GPUI frontend's does. The mapping therefore resolves against holon's DEFAULT
//! dark theme, and says so.
//!
//! That is still strictly better than the table it replaces. That one lived in
//! `holon_api::render_eval` and knew only CSS colour names and one theme name,
//! so `muted` painted a fixed grey and `primary` painted white, and an unknown
//! name was indistinguishable from either. The match below is exhaustive over
//! [`ThemeToken`], so a token added to the vocabulary fails to compile here
//! instead of quietly painting white.
//!
//! The GPUI frontend is the reference implementation
//! (`frontends/gpui/src/render/builders/theme.rs`); the two agree token for
//! token, and the only difference is which theme the slot is read from.

use holon_api::theme_token::ThemeToken;
use holon_frontend::theme::Rgba8;
use holon_frontend::theme::ThemeColors;

use super::prelude::*;

/// The `#RRGGBB` a token names, in this frontend's palette.
pub(crate) fn theme_token_hex(token: ThemeToken) -> String {
    let c = ThemeColors::default_dark();
    let rgba: Rgba8 = match token {
        ThemeToken::Accent => c.primary,
        ThemeToken::Error => c.error,
        ThemeToken::Foreground => c.text_primary,
        ThemeToken::Info => c.primary_light,
        ThemeToken::Muted | ThemeToken::Secondary => c.text_secondary,
        ThemeToken::Primary => c.primary,
        ThemeToken::Success => c.success,
        ThemeToken::Warning => c.warning,
    };
    format!("#{:02X}{:02X}{:02X}", rgba[0], rgba[1], rgba[2])
}

/// The colour a `color` prop names, or `None` when the prop is absent or empty.
pub(crate) fn optional_colour_prop(name: Option<&str>) -> Option<Color> {
    let name = name?;
    if name.is_empty() {
        return None;
    }
    match ThemeToken::parse(name) {
        Ok(token) => Some(Color::srgb_hex(&theme_token_hex(token))),
        // Unreachable by construction: `holon_api::render_dsl` refuses a colour
        // name the theme does not define when the layout doc is parsed, and the
        // shared shadow builders refuse it again at build time. This frontend
        // declares no logging crate at all, so the disclosure is the assertion
        // below in a debug build plus the widget's own default colour in a
        // release one, rather than a dependency added to reach a branch that
        // cannot be taken. The GPUI frontend has `tracing` and logs instead.
        Err(_) => {
            debug_assert!(false, "a colour name reached the waterui paint unparsed");
            None
        }
    }
}

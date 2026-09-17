//! The one place a theme token becomes a pixel in this frontend.
//!
//! waterui wires no live theme, so the mapping resolves against holon's DEFAULT
//! dark theme rather than the user's choice, and says so. The match is
//! exhaustive over [`ThemeToken`], so a token added to the vocabulary fails to
//! compile here rather than reaching a catch-all.
//!
//! The GPUI frontend's resolver (`frontends/gpui/src/render/builders/theme.rs`)
//! maps the same nine tokens; the only difference between the two is which
//! theme the slot is read from.

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
///
/// An `Err` means the value names no colour this theme defines, which a row can
/// supply (`#{color: col("kind")}`). The caller renders it: this frontend has
/// no error widget, so its convention for a refused widget is the message in
/// red (see `builders/mod.rs`), and a build that can show it must, rather than
/// dropping the colour and painting on.
pub(crate) fn optional_colour_prop(name: Option<&str>) -> Result<Option<Color>, String> {
    let Some(name) = name else {
        return Ok(None);
    };
    if name.is_empty() {
        return Ok(None);
    }
    let token = holon_frontend::theme_arg::resolve_colour_arg(name)?;
    Ok(Some(Color::srgb_hex(&theme_token_hex(token))))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_resolves_to_its_colour() {
        for token in ThemeToken::ALL {
            let colour = optional_colour_prop(Some(token.as_str()))
                .unwrap_or_else(|e| panic!("{token:?} must resolve, got {e}"));
            assert!(colour.is_some(), "{token:?} must produce a colour");
        }
    }

    #[test]
    fn an_absent_or_empty_name_is_not_a_colour() {
        assert!(optional_colour_prop(None).unwrap().is_none());
        assert!(optional_colour_prop(Some("")).unwrap().is_none());
    }

    /// The row-value case. `text(col("content"), #{color: col("kind")})` hands
    /// this the row's value, which the theme may have no slot for.
    #[test]
    fn a_value_the_theme_does_not_define_is_refused() {
        let err = optional_colour_prop(Some("corrigendum"))
            .expect_err("a value the theme has no slot for must be refused, not painted");
        assert!(err.contains("corrigendum"), "{err}");
    }
}

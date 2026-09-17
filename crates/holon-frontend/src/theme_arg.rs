//! Where a colour argument becomes a theme token, once for every frontend.
//!
//! A colour argument's value has two sources, and they are checked in different
//! places for the same reason.
//!
//! A layout LITERAL (`#{color: "muted"}`) is refused while the doc is parsed
//! (`holon_api::render_dsl`), so by the time anything renders it is a name the
//! theme defines or the doc never loaded.
//!
//! A ROW value (`#{color: col("kind")}`) cannot be judged then: which name it
//! holds is a property of the data. It becomes a token here instead, so the
//! frontends cannot disagree about which names are colours. A value that is not
//! a token is an `Err` naming it. What a caller does with that `Err` depends on
//! what it can show: an error widget where one exists (the shared builders, and
//! waterui's red error text), or a logged default inside a paint pass, which
//! has no error channel. Both disclose the value; neither substitutes quietly.

use holon_api::theme_token::ThemeToken;

/// Parse a colour argument's value into a theme token.
pub fn resolve_colour_arg(raw: &str) -> Result<ThemeToken, String> {
    ThemeToken::parse(raw).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_token_resolves_to_itself() {
        for token in ThemeToken::ALL {
            assert_eq!(resolve_colour_arg(token.as_str()).unwrap(), token);
        }
    }

    /// The row-value case: a value the theme has no slot for is refused with
    /// the value in the message, never passed on as a colour.
    #[test]
    fn a_value_the_theme_does_not_define_is_refused() {
        let err = resolve_colour_arg("corrigendum").unwrap_err();
        assert!(err.contains("corrigendum"), "{err}");
        assert!(err.contains("THEME_TOKENS"), "{err}");
    }
}

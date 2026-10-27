//! The colour names a layout may ask a widget for.
//!
//! A colour in the render DSL is a plain string argument, and this is the one
//! vocabulary such a name is checked against. A literal is refused where the
//! layout doc is parsed (`render_dsl::validate_render_expr`); a value a row
//! supplies is refused where it is resolved (`holon_frontend::theme_arg`).
//! Each frontend then resolves a token through ONE token-to-pixel function, so
//! one name cannot come out as two colours in two places.
//!
//! The set is deliberately small and semantic. It holds no hex arm and no CSS
//! colour names: a layout that wants `#3B82F6` wants a colour that does not
//! follow the active theme, and both a light and a dark theme ship. The theme
//! files under `assets/themes/` are where literal colours belong.
//!
//! A frontend-side test asserts this table and its renderer agree, so a
//! renderer that learns a token without listing it here fails at build time
//! rather than silently putting a valid name out of a layout's reach.

use std::fmt;

/// Every colour a layout may name, as the names themselves, sorted.
///
/// These are the strings a layout writes; [`ThemeToken`] is the same list as a
/// type, and a test asserts the two agree, so a token added to only one of them
/// fails loudly.
///
/// `muted` and `secondary` are separate entries that every frontend resolves to
/// one pixel. They stay separate because parsing is accept-or-refuse and never
/// rewrites: a parser that turned `secondary` into `muted` would give one
/// expression two props that disagree, since a props-only widget's fast path
/// re-reads this name without going through the parser.
pub const THEME_TOKENS: &[&str] = &[
    "accent",
    "error",
    "foreground",
    "info",
    "muted",
    "primary",
    "secondary",
    "success",
    "warning",
];

/// A colour name [`THEME_TOKENS`] holds, as a type.
///
/// An enum rather than a string newtype, so each frontend resolves it with an
/// EXHAUSTIVE match. A frontend that misses a token then fails to compile
/// instead of quietly painting its catch-all, which is the defect this module
/// exists to remove: the old resolvers all ended in `_ => foreground`, so
/// `color: "primary"` painted body text and said nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ThemeToken {
    Accent,
    Error,
    Foreground,
    Info,
    Muted,
    Primary,
    Secondary,
    Success,
    Warning,
}

impl ThemeToken {
    /// Every token, for iterating in a test or a palette.
    pub const ALL: [ThemeToken; 9] = [
        ThemeToken::Accent,
        ThemeToken::Error,
        ThemeToken::Foreground,
        ThemeToken::Info,
        ThemeToken::Muted,
        ThemeToken::Primary,
        ThemeToken::Secondary,
        ThemeToken::Success,
        ThemeToken::Warning,
    ];

    /// Parse a colour name. Accept-or-refuse, never a rewrite: see
    /// [`THEME_TOKENS`] for why the two are incompatible.
    pub fn parse(raw: &str) -> Result<Self, UnknownThemeToken> {
        Self::ALL
            .iter()
            .copied()
            .find(|token| token.as_str() == raw)
            .ok_or_else(|| UnknownThemeToken {
                raw: raw.to_string(),
            })
    }

    pub const fn as_str(self) -> &'static str {
        match self {
            ThemeToken::Accent => "accent",
            ThemeToken::Error => "error",
            ThemeToken::Foreground => "foreground",
            ThemeToken::Info => "info",
            ThemeToken::Muted => "muted",
            ThemeToken::Primary => "primary",
            ThemeToken::Secondary => "secondary",
            ThemeToken::Success => "success",
            ThemeToken::Warning => "warning",
        }
    }
}

impl fmt::Display for ThemeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownThemeToken {
    pub raw: String,
}

impl fmt::Display for UnknownThemeToken {
    /// Names the near misses rather than the whole table: an author who typed
    /// `sucess` wants to be told `success` exists, and a nine-entry dump
    /// would bury that.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let lowered = self.raw.to_lowercase();
        let stem: String = lowered.chars().take(3).collect();
        let near: Vec<&str> = THEME_TOKENS
            .iter()
            .copied()
            .filter(|t| !stem.is_empty() && (t.starts_with(&stem) || lowered.contains(*t)))
            .take(4)
            .collect();
        write!(f, "unknown theme token {:?}", self.raw)?;
        if near.is_empty() {
            write!(
                f,
                "; the theme defines {} tokens, listed in \
                 holon_api::theme_token::THEME_TOKENS",
                THEME_TOKENS.len()
            )
        } else {
            write!(
                f,
                "; did you mean one of {near:?}? The full list is \
                 holon_api::theme_token::THEME_TOKENS"
            )
        }
    }
}

impl std::error::Error for UnknownThemeToken {}

impl serde::Serialize for ThemeToken {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> serde::Deserialize<'de> for ThemeToken {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests;

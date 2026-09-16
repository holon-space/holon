//! The colour names a layout may ask a widget for.
//!
//! A colour in the render DSL is a plain string argument, and nothing validated
//! it: `text(..., #{color: "corrigendum"})` built a widget carrying the
//! nonsense name, and the frontend's colour resolver only met it inside the
//! frame loop, where the only options are a panic or a silent substitution.
//! Every frontend had grown its own substitution table, and the tables
//! disagreed: `text(color: "primary")` painted the body foreground,
//! `card(accent: "primary")` painted grey, and neither said so.
//!
//! So the vocabulary is parsed here, at the config boundary, where a typo is a
//! refusal rather than a substitution. Each frontend keeps ONE token-to-pixel
//! function over this table instead of its own string match.
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

/// Every colour a layout may name, sorted.
///
/// `muted` and `secondary` are both here, and every frontend resolves the two
/// to one pixel. They are two names rather than one because the shipped
/// resolvers already accepted both, and because parsing must be a pure
/// accept-or-refuse: a parser that REWROTE `secondary` into `muted` would make
/// the props derived from one expression disagree with each other, since a
/// props-only widget's fast path re-reads this name from the expression
/// without going through the validator.
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

/// A colour name [`THEME_TOKENS`] holds — proof that a frontend can resolve it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ThemeToken(&'static str);

impl ThemeToken {
    /// Parse a colour name. Accept-or-refuse, never a rewrite: see
    /// [`THEME_TOKENS`] for why the two are incompatible.
    pub fn parse(raw: &str) -> Result<Self, UnknownThemeToken> {
        THEME_TOKENS
            .iter()
            .find(|t| **t == raw)
            .map(|t| Self(t))
            .ok_or_else(|| UnknownThemeToken {
                raw: raw.to_string(),
            })
    }

    pub fn as_str(&self) -> &'static str {
        self.0
    }
}

impl fmt::Display for ThemeToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownThemeToken {
    pub raw: String,
}

impl fmt::Display for UnknownThemeToken {
    /// Names the near misses rather than the whole table: an author who typed
    /// `sucess` wants to be told `success` exists, and an eight-entry dump
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
        serializer.serialize_str(self.0)
    }
}

impl<'de> serde::Deserialize<'de> for ThemeToken {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(deserializer)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_sorted_and_free_of_duplicates() {
        let mut sorted = THEME_TOKENS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted, THEME_TOKENS.to_vec());
    }

    #[test]
    fn every_token_parses_to_itself() {
        for token in THEME_TOKENS {
            assert_eq!(ThemeToken::parse(token).unwrap().as_str(), *token);
        }
    }

    /// Parsing never rewrites a name. A props-only widget's fast path re-reads
    /// the colour from the expression without passing the validator, so a
    /// parser that canonicalised would have the two paths write different
    /// strings into the same prop.
    #[test]
    fn parsing_never_rewrites_the_name() {
        for token in THEME_TOKENS {
            assert_eq!(ThemeToken::parse(token).unwrap().as_str(), *token);
        }
        assert_eq!(
            ThemeToken::parse("secondary").unwrap().as_str(),
            "secondary"
        );
    }

    #[test]
    fn an_unlisted_name_is_refused_and_the_message_names_it() {
        let err = ThemeToken::parse("corrigendum").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("\"corrigendum\""), "{msg}");
        assert!(msg.contains("THEME_TOKENS"), "{msg}");
    }

    #[test]
    fn a_near_miss_is_named_in_the_refusal() {
        let msg = ThemeToken::parse("sucess").unwrap_err().to_string();
        assert!(msg.contains("success"), "{msg}");
    }

    /// The set deliberately holds no literal colour. A hex that parsed here
    /// would be a colour that ignores the active theme, and this is the
    /// boundary that is supposed to make that impossible.
    #[test]
    fn a_literal_colour_is_refused() {
        assert!(ThemeToken::parse("#3B82F6").is_err());
        assert!(ThemeToken::parse("red").is_err());
        assert!(ThemeToken::parse("gray").is_err());
    }

    #[test]
    fn parsing_is_case_sensitive_because_the_renderers_are() {
        assert!(ThemeToken::parse("Muted").is_err());
        assert!(ThemeToken::parse("Primary").is_err());
    }
}

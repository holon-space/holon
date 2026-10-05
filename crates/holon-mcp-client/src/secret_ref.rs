//! A sidecar's auth value, parsed so it cannot BE a secret.
//!
//! The rule a sidecar author has to follow is one sentence: an auth value is
//! an optional literal prefix followed by exactly one `${VAR}`. A value with
//! no reference in it is a credential somebody typed into a file, and a file
//! is copied, synced, committed and pasted into bug reports.
//!
//! Parsed at deserialization, so the refusal lands where the file is read and
//! no later code has a `String` it must remember to check. Nothing here
//! matches token FORMATS — the absence of a reference is the defect, which
//! keeps the rule true for the next provider's token shape.

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;

/// One `${VAR}` reference, with whatever literal precedes it.
///
/// `Bearer ${GITHUB_TOKEN}` is `prefix: "Bearer ", var: "GITHUB_TOKEN"`.
/// There is deliberately no suffix: a literal AFTER the reference is how a
/// secret gets past a prefix-only rule, and no scheme this transport speaks
/// needs one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SecretRef {
    prefix: String,
    var: String,
}

impl SecretRef {
    /// Parse an auth header VALUE (`holon.auth.value`), or say what shape was
    /// required.
    ///
    /// The message never quotes the value: the whole point of the refusal is
    /// that the value may be a live credential, and an error string is written
    /// to a log.
    pub fn parse(raw: &str) -> Result<Self, SecretRefError> {
        Self::parse_in(raw, SecretField::HeaderValue)
    }

    fn parse_in(raw: &str, field: SecretField) -> Result<Self, SecretRefError> {
        let refuse = |kind| SecretRefError { field, kind };
        let open = raw
            .find("${")
            .ok_or(refuse(SecretRefErrorKind::NoReference))?;
        let after = &raw[open + 2..];
        let close = after
            .find('}')
            .ok_or(refuse(SecretRefErrorKind::Unterminated))?;
        let var = &after[..close];
        let rest = &after[close + 1..];

        if !rest.is_empty() {
            return Err(refuse(SecretRefErrorKind::TrailingText));
        }
        if var.is_empty() {
            return Err(refuse(SecretRefErrorKind::EmptyName));
        }
        let prefix = &raw[..open];
        if prefix.contains("${") {
            return Err(refuse(SecretRefErrorKind::TrailingText));
        }
        Ok(Self {
            prefix: prefix.to_string(),
            var: var.to_string(),
        })
    }

    /// The variable this value references.
    pub fn var(&self) -> &str {
        &self.var
    }

    /// The literal that precedes the reference (`"Bearer "`, often empty).
    pub fn prefix(&self) -> &str {
        &self.prefix
    }

    /// The value as written, for serializing a sidecar back out. Safe: it
    /// holds a variable NAME, which is what makes this type worth having.
    pub fn as_written(&self) -> String {
        format!("{}${{{}}}", self.prefix, self.var)
    }
}

/// The `auth.static_token` field: exactly one bare `${VAR}`.
///
/// A distinct type from [`SecretRef`] because the HTTP transport adds the
/// `Bearer ` scheme itself, so a literal prefix here would be sent as
/// `Bearer Bearer <token>`. The prefix cannot be represented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TokenRef {
    var: String,
}

impl TokenRef {
    pub fn parse(raw: &str) -> Result<Self, SecretRefError> {
        let r = SecretRef::parse_in(raw, SecretField::StaticToken)?;
        if !r.prefix.is_empty() {
            return Err(SecretRefError {
                field: SecretField::StaticToken,
                kind: SecretRefErrorKind::PrefixOnBearerToken,
            });
        }
        Ok(Self { var: r.var })
    }

    pub fn var(&self) -> &str {
        &self.var
    }

    /// The value as written (`${VAR}`), for env expansion and serializing.
    pub fn as_written(&self) -> String {
        format!("${{{}}}", self.var)
    }
}

/// Which sidecar field an auth value was written in. Decides the example the
/// refusal gives: the right shape differs per field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretField {
    /// `auth.static_token` of an MCP HTTP sidecar.
    StaticToken,
    /// `holon.auth.value` of a `utcp:` sidecar's static header.
    HeaderValue,
}

/// Why an auth value is not a reference. No variant carries the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecretRefErrorKind {
    NoReference,
    Unterminated,
    TrailingText,
    EmptyName,
    PrefixOnBearerToken,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SecretRefError {
    pub field: SecretField,
    pub kind: SecretRefErrorKind,
}

impl std::fmt::Display for SecretRefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (field, shape, example) = match self.field {
            SecretField::StaticToken => (
                "auth.static_token",
                "exactly ONE bare ${VAR} reference with no text around it",
                "static_token: \"${GITHUB_TOKEN}\"",
            ),
            SecretField::HeaderValue => (
                "holon.auth.value",
                "an optional literal prefix followed by exactly ONE ${VAR} reference",
                "value: \"Bearer ${GITHUB_TOKEN}\"",
            ),
        };
        let why = match self.kind {
            SecretRefErrorKind::NoReference => {
                "it holds no ${VAR} reference, so it is a secret written into the file"
            }
            SecretRefErrorKind::Unterminated => "its ${ is never closed by a '}'",
            SecretRefErrorKind::TrailingText => {
                "it holds more than one ${VAR}, or text after the reference"
            }
            SecretRefErrorKind::EmptyName => "its ${} names no variable",
            SecretRefErrorKind::PrefixOnBearerToken => {
                "it has text before the reference, but the HTTP transport adds \"Bearer \" \
                 itself, so a prefix would be sent as \"Bearer Bearer <token>\""
            }
        };
        write!(
            f,
            "{field} must be {shape} (for example {example}), but {why}. The value is not \
             quoted here because it may be a live credential. Put the secret in the keychain or \
             the environment and reference it by name."
        )
    }
}

impl std::error::Error for SecretRefError {}

impl<'de> Deserialize<'de> for SecretRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl Serialize for SecretRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_written())
    }
}

impl<'de> Deserialize<'de> for TokenRef {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Self::parse(&raw).map_err(serde::de::Error::custom)
    }
}

impl Serialize for TokenRef {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.as_written())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(kind: SecretRefErrorKind) -> SecretRefError {
        SecretRefError {
            field: SecretField::HeaderValue,
            kind,
        }
    }

    #[test]
    fn a_token_ref_refuses_a_prefix_and_keeps_only_the_variable() {
        assert_eq!(TokenRef::parse("${T}").expect("parses").var(), "T");
        assert_eq!(
            TokenRef::parse("Bearer ${T}"),
            Err(SecretRefError {
                field: SecretField::StaticToken,
                kind: SecretRefErrorKind::PrefixOnBearerToken
            })
        );
    }

    #[test]
    fn a_bare_reference_has_an_empty_prefix() {
        let r = SecretRef::parse("${TODOIST_API_KEY}").expect("parses");
        assert_eq!(r.prefix(), "");
        assert_eq!(r.var(), "TODOIST_API_KEY");
        assert_eq!(r.as_written(), "${TODOIST_API_KEY}");
    }

    #[test]
    fn a_prefixed_reference_keeps_its_prefix() {
        let r = SecretRef::parse("Bearer ${GITHUB_TOKEN}").expect("parses");
        assert_eq!(r.prefix(), "Bearer ");
        assert_eq!(r.var(), "GITHUB_TOKEN");
        assert_eq!(r.as_written(), "Bearer ${GITHUB_TOKEN}");
    }

    #[test]
    fn the_refusals() {
        assert_eq!(
            SecretRef::parse("literal"),
            Err(err(SecretRefErrorKind::NoReference))
        );
        assert_eq!(
            SecretRef::parse(""),
            Err(err(SecretRefErrorKind::NoReference))
        );
        assert_eq!(
            SecretRef::parse("${X"),
            Err(err(SecretRefErrorKind::Unterminated))
        );
        assert_eq!(
            SecretRef::parse("${}"),
            Err(err(SecretRefErrorKind::EmptyName))
        );
        assert_eq!(
            SecretRef::parse("${A}${B}"),
            Err(err(SecretRefErrorKind::TrailingText))
        );
        assert_eq!(
            SecretRef::parse("${A}tail"),
            Err(err(SecretRefErrorKind::TrailingText))
        );
    }

    /// The message is written into logs, so it must be safe to log.
    #[test]
    fn no_refusal_quotes_the_value() {
        let secret = "SYNTHETIC-NOT-A-REAL-TOKEN";
        let msg = SecretRef::parse(secret).expect_err("refused").to_string();
        assert!(!msg.contains(secret), "got: {msg}");
    }
}

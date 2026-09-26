//! Keys and values of org property carriers: the `:PROPERTIES:` drawer lines
//! and a source block's `:key value` header arguments.
//!
//! A value is written raw when the parser reads it back unchanged, else as a
//! JSON string literal. The parser decodes a literal only when it is exactly
//! what the encoder writes for its content, so a quoted value a person typed
//! (`"The Book"`) stays as typed. Both directions together are lossless for
//! every string.

use std::borrow::Cow;
use std::fmt;

/// A property key org can write on a `:key: value` line and read back as the
/// same key: one token with no whitespace, no `:`, no control character; not
/// a drawer delimiter; not ending in `+` (org's append syntax, which orgize
/// reads as the key without it); and not a spelling of `ID`, which the parser
/// takes as the block's identity (see [`DrawerId`]).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DrawerKey(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrepresentableKey {
    pub key: String,
}

impl fmt::Display for UnrepresentableKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "property key {:?} cannot be written to an org drawer: a key is one token with no \
             whitespace, no ':', no control character, no trailing '+', and is not PROPERTIES, \
             END or a spelling of ID",
            self.key
        )
    }
}

impl std::error::Error for UnrepresentableKey {}

impl DrawerKey {
    pub fn parse(raw: &str) -> Result<DrawerKey, UnrepresentableKey> {
        let legal = !raw.is_empty()
            && !raw
                .chars()
                .any(|c| c.is_whitespace() || c.is_control() || c == ':')
            && !raw.ends_with('+')
            && !["PROPERTIES", "END", "ID"]
                .iter()
                .any(|reserved| raw.eq_ignore_ascii_case(reserved));
        if legal {
            Ok(DrawerKey(raw.to_string()))
        } else {
            Err(UnrepresentableKey {
                key: raw.to_string(),
            })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// The value of an `:ID:` line: a block's bare id as the org parser reads it
/// back, a non-empty token that forms a URI. It is written raw, never through
/// the value codec, because the parser takes the id line verbatim.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DrawerId(String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrepresentableId {
    pub value: String,
}

impl fmt::Display for UnrepresentableId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "the id {:?} cannot be written to an org :ID: line: an id is one non-empty token \
             that forms a URI (no whitespace, no line break)",
            self.value
        )
    }
}

impl std::error::Error for UnrepresentableId {}

impl DrawerId {
    pub fn parse(raw: &str) -> Result<DrawerId, UnrepresentableId> {
        let legal = !raw.is_empty()
            && ValueCarrier::HeadlineDrawer.encode(raw) == raw
            && holon_api::EntityUri::try_from_raw(raw).is_ok();
        if legal {
            Ok(DrawerId(raw.to_string()))
        } else {
            Err(UnrepresentableId {
                value: raw.to_string(),
            })
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A place org writes a property value. Each has its own parser, so each
/// has its own set of texts that survive raw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueCarrier {
    /// A headline's `:PROPERTIES:` drawer: orgize trims the value and drops
    /// a line whose value is empty.
    HeadlineDrawer,
    /// The file-level drawer: Holon's own line reader trims the value and
    /// keeps an empty one.
    FileDrawer,
    /// A source block's `:key value` header arguments: split on whitespace,
    /// and a token that starts with `:` begins the next key.
    HeaderArg,
}

impl ValueCarrier {
    pub fn encode(self, value: &str) -> Cow<'_, str> {
        if self.survives_raw(value) && self.decode(value) == value {
            Cow::Borrowed(value)
        } else {
            Cow::Owned(self.literal(value))
        }
    }

    pub fn decode(self, text: &str) -> Cow<'_, str> {
        if text.starts_with('"') {
            if let Ok(value) = serde_json::from_str::<String>(text) {
                if self.encode(&value) == text {
                    return Cow::Owned(value);
                }
            }
        }
        Cow::Borrowed(text)
    }

    fn survives_raw(self, s: &str) -> bool {
        let has_control = s.chars().any(|c| c.is_control() && c != '\t');
        match self {
            ValueCarrier::HeadlineDrawer => !s.is_empty() && s.trim() == s && !has_control,
            ValueCarrier::FileDrawer => s.trim() == s && !has_control,
            ValueCarrier::HeaderArg => {
                let tokens: Vec<&str> = s.split_whitespace().collect();
                tokens.join(" ") == s && !tokens.iter().any(|t| t.starts_with(':')) && !has_control
            }
        }
    }

    /// The JSON string literal of `s`. Header arguments also escape every
    /// whitespace character, so the literal stays one token.
    fn literal(self, s: &str) -> String {
        let json = serde_json::to_string(s).expect("a str serializes to JSON");
        if self != ValueCarrier::HeaderArg {
            return json;
        }
        let mut out = String::with_capacity(json.len());
        for c in json.chars() {
            if c.is_whitespace() {
                out.push_str(&format!("\\u{:04x}", c as u32));
            } else {
                out.push(c);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::DrawerId;
    use super::DrawerKey;
    use super::ValueCarrier::*;

    #[test]
    fn keys_that_read_back_as_another_key_are_refused() {
        for key in ["note+", "+", "ID", "id", "Id", "iD", "end", "Properties"] {
            assert!(DrawerKey::parse(key).is_err(), "{key:?} was accepted");
        }
        for key in ["note", "IDENT", "a+b", "Effort"] {
            assert!(DrawerKey::parse(key).is_ok(), "{key:?} was refused");
        }
    }

    #[test]
    fn an_id_is_a_non_empty_uri_token() {
        for id in [
            "abc-123",
            "550e8400-e29b-41d4-a716-446655440000",
            "12:34",
            "block:abc",
        ] {
            assert!(DrawerId::parse(id).is_ok(), "{id:?} was refused");
        }
        for id in ["", "a b", "kid\n* Evil", " kid", "a\"b", "\"kid\""] {
            assert!(DrawerId::parse(id).is_err(), "{id:?} was accepted");
        }
    }

    #[test]
    fn plain_text_is_written_raw() {
        assert_eq!(HeadlineDrawer.encode("high"), "high");
        assert_eq!(HeadlineDrawer.encode("a \"b\" c"), "a \"b\" c");
    }

    #[test]
    fn a_quoted_value_a_person_typed_stays_as_typed() {
        assert_eq!(HeadlineDrawer.decode("\"The Book\""), "\"The Book\"");
        assert_eq!(HeadlineDrawer.encode("\"The Book\""), "\"The Book\"");
    }

    #[test]
    fn values_the_carrier_cannot_hold_become_literals() {
        assert_eq!(HeadlineDrawer.encode("a\nb"), "\"a\\nb\"");
        assert_eq!(HeadlineDrawer.encode(" padded "), "\" padded \"");
        assert_eq!(HeadlineDrawer.encode(""), "\"\"");
        assert_eq!(FileDrawer.encode(""), "");
        assert_eq!(FileDrawer.decode("\"\""), "\"\"");
        assert_eq!(HeaderArg.encode("a b"), "a b");
        assert_eq!(HeaderArg.encode("a  b"), "\"a\\u0020\\u0020b\"");
        assert_eq!(HeaderArg.encode("x :k"), "\"x\\u0020:k\"");
    }

    #[test]
    fn a_literal_of_a_raw_safe_value_is_itself_encoded() {
        let nested = HeadlineDrawer.encode("\"a\\nb\"");
        assert_eq!(HeadlineDrawer.decode(&nested), "\"a\\nb\"");
    }
}

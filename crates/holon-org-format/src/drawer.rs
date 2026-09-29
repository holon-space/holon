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

use holon_api::EdgeField;

/// A property key one carrier's reader reads back as the same key, and not as
/// the carrier's identity. Each carrier has its own reader, so a key is only
/// valid for the carrier it was parsed for: see [`ValueCarrier::key`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DrawerKey {
    carrier: ValueCarrier,
    key: String,
}

impl DrawerKey {
    pub fn as_str(&self) -> &str {
        &self.key
    }

    /// `value` as this key's carrier writes it.
    pub fn encode<'a>(&self, value: &'a str) -> Cow<'a, str> {
        self.carrier.encode(value)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnrepresentableKey {
    pub key: String,
    pub carrier: ValueCarrier,
}

impl fmt::Display for UnrepresentableKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let rule = match self.carrier {
            ValueCarrier::HeadlineDrawer => {
                "a headline :PROPERTIES: drawer: a key there is one token with no whitespace, no \
                 ':', no control character, no trailing '+', and is not PROPERTIES, END or a \
                 spelling of ID"
            }
            ValueCarrier::FileDrawer => {
                "the file-level :PROPERTIES: drawer: a key there is one token with no whitespace \
                 and no ':', and is not PROPERTIES, END or a spelling of ID"
            }
            ValueCarrier::HeaderArg => {
                "a source block's header arguments: a key there is one token with no whitespace, \
                 and is not `id`"
            }
        };
        write!(f, "property key {:?} cannot be written to {rule}", self.key)
    }
}

impl std::error::Error for UnrepresentableKey {}

/// A property key as a file wrote it. A block's property bag keeps the keys
/// that start with `_` for Holon's own (the parser's carriers, `_provenance`),
/// so there an authored key that starts with `_` or `\` is stored behind a `\`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct AuthoredKey(String);

impl AuthoredKey {
    const ESCAPE: char = '\\';

    pub fn new(key: &str) -> Self {
        Self(key.to_string())
    }

    /// The authored key a property-bag key stores; `None` for Holon's own.
    pub fn from_property(key: &str) -> Option<Self> {
        match key.strip_prefix(Self::ESCAPE) {
            Some(authored) => Some(Self(authored.to_string())),
            None => (!key.starts_with('_')).then(|| Self(key.to_string())),
        }
    }

    /// The key in a block's property bag.
    pub fn property(&self) -> String {
        if self.0.starts_with(['_', Self::ESCAPE]) {
            format!("{}{}", Self::ESCAPE, self.0)
        } else {
            self.0.clone()
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A block's bare id as org carries it — on a headline's `:ID:` line, a source
/// block's `:id` header argument, or a page's `#+ID:` — and reads it back as
/// `block:<id>` with the same id. It names no URI scheme of its own (`doc:x`,
/// `block:x`), since a reader could not tell it from a schemed reference, and
/// it is at most [`DrawerId::MAX_LEN`] bytes.
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
            "the id {:?} cannot be written to an org id line: an id there is a bare block id \
             of at most {} bytes that forms the URI `block:<id>` with the same id, names no URI \
             scheme of its own and does not start with ':'",
            self.value,
            DrawerId::MAX_LEN
        )
    }
}

impl std::error::Error for UnrepresentableId {}

impl DrawerId {
    pub const MAX_LEN: usize = 255;

    pub fn parse(raw: &str) -> Result<DrawerId, UnrepresentableId> {
        let legal = !raw.is_empty()
            && raw.len() <= Self::MAX_LEN
            && holon_api::EntityUri::schemed(raw).is_none()
            && holon_api::EntityUri::try_from_raw(raw).is_ok_and(|u| u.is_block() && u.id() == raw)
            && crate::models::parse_header_args_from_str(&format!(":id {raw}"))
                .into_iter()
                .eq([("id".to_string(), raw.to_string())]);
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

/// A headline drawer key the org parser reads into a typed block field, never
/// into the block's properties. Org drawer keys are case-insensitive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypedDrawerKey {
    Id,
    /// `:REQUIRES:` and `:BLOCKED-BY:`, one `requires` edge.
    Dependency,
    /// Any other edge field, in its column or kebab spelling.
    Edge(EdgeField),
    Priority,
    Collapsed,
    WidgetOnly,
    /// Refuses the file: a task state is the headline keyword.
    TaskState,
}

impl TypedDrawerKey {
    pub fn parse(key: &str) -> Option<Self> {
        Self::spellings()
            .iter()
            .find(|(spelling, _)| key.eq_ignore_ascii_case(spelling))
            .map(|(_, typed)| *typed)
    }

    /// Every key [`Self::parse`] names, each in one case.
    pub fn spellings() -> &'static [(String, Self)] {
        static SPELLINGS: std::sync::LazyLock<Vec<(String, TypedDrawerKey)>> =
            std::sync::LazyLock::new(|| {
                let mut all: Vec<(String, TypedDrawerKey)> = [
                    ("ID", TypedDrawerKey::Id),
                    ("REQUIRES", TypedDrawerKey::Dependency),
                    ("BLOCKED-BY", TypedDrawerKey::Dependency),
                    (crate::models::org_props::PRIORITY, TypedDrawerKey::Priority),
                    ("COLLAPSED", TypedDrawerKey::Collapsed),
                    ("WIDGET_ONLY", TypedDrawerKey::WidgetOnly),
                    ("task_state", TypedDrawerKey::TaskState),
                    ("task_state_category", TypedDrawerKey::TaskState),
                ]
                .into_iter()
                .map(|(spelling, typed)| (spelling.to_string(), typed))
                .collect();
                for edge in EdgeField::ALL {
                    if edge == EdgeField::Requires {
                        continue;
                    }
                    let column = edge.column();
                    all.push((column.to_string(), TypedDrawerKey::Edge(edge)));
                    if column.contains('_') {
                        all.push((column.replace('_', "-"), TypedDrawerKey::Edge(edge)));
                    }
                }
                all
            });
        &SPELLINGS
    }
}

/// A place org writes a property. Each has its own reader, so each has its
/// own set of keys and of value texts that survive raw.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ValueCarrier {
    /// A headline's `:PROPERTIES:` drawer: Holon's line reader trims the
    /// value and keeps an empty one.
    HeadlineDrawer,
    /// The file-level drawer: Holon's own line reader trims the value and
    /// keeps an empty one.
    FileDrawer,
    /// A source block's `:key value` header arguments: split on whitespace,
    /// and a token that starts with `:` begins the next key.
    HeaderArg,
}

impl ValueCarrier {
    pub fn key(self, raw: &str) -> Result<DrawerKey, UnrepresentableKey> {
        let reads_back = match self {
            ValueCarrier::HeadlineDrawer => {
                !raw.is_empty()
                    && !raw
                        .chars()
                        .any(|c| c.is_whitespace() || c.is_control() || c == ':')
                    && !raw.ends_with('+')
                    && !["PROPERTIES", "END", "ID"]
                        .iter()
                        .any(|reserved| raw.eq_ignore_ascii_case(reserved))
            }
            ValueCarrier::FileDrawer => {
                crate::parser::parse_drawer_line(&format!(":{raw}: v"))
                    .is_some_and(|(key, _)| key == raw)
                    && !raw.eq_ignore_ascii_case("ID")
            }
            ValueCarrier::HeaderArg => {
                !raw.is_empty()
                    && raw != "id"
                    && crate::models::parse_header_args_from_str(&format!(":{raw} v"))
                        .into_iter()
                        .eq([(raw.to_string(), "v".to_string())])
            }
        };
        if reads_back {
            Ok(DrawerKey {
                carrier: self,
                key: raw.to_string(),
            })
        } else {
            Err(UnrepresentableKey {
                key: raw.to_string(),
                carrier: self,
            })
        }
    }

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
            ValueCarrier::HeadlineDrawer | ValueCarrier::FileDrawer => {
                s.trim() == s && !has_control
            }
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
    use super::ValueCarrier::*;

    #[test]
    fn each_carrier_takes_the_keys_its_reader_reads_back() {
        let cases: [(&str, [bool; 3]); 12] = [
            ("note", [true, true, true]),
            ("IDENT", [true, true, true]),
            ("a+b", [true, true, true]),
            ("note+", [false, true, true]),
            ("+", [false, true, true]),
            ("Id", [false, false, true]),
            ("ID", [false, false, true]),
            ("id", [false, false, false]),
            ("end", [false, false, true]),
            ("a:b", [false, false, true]),
            ("a b", [false, false, false]),
            ("", [false, false, false]),
        ];
        for (key, [headline, file, header]) in cases {
            assert_eq!(
                HeadlineDrawer.key(key).is_ok(),
                headline,
                "headline {key:?}"
            );
            assert_eq!(FileDrawer.key(key).is_ok(), file, "file drawer {key:?}");
            assert_eq!(
                HeaderArg.key(key).is_ok(),
                header,
                "header argument {key:?}"
            );
        }
    }

    #[test]
    fn an_id_is_a_bare_block_id() {
        for id in [
            "abc-123",
            "550e8400-e29b-41d4-a716-446655440000",
            "12:34",
            "journals::auto-create",
            "lessons_for_tasks::rule::0",
            "a.b_c~d",
            "a/b",
            "Notes/Sub.md::b::0",
            "a%20b",
            "a=b",
            "a:::b",
            "kid+",
            "*kid",
        ] {
            assert!(DrawerId::parse(id).is_ok(), "{id:?} was refused");
        }
        let too_long = "x".repeat(5000);
        for id in [
            "",
            "a b",
            "kid\n* Evil",
            " kid",
            "a\"b",
            "\"kid\"",
            "block:abc",
            "doc:abc",
            "file:abc",
            "sentinel:no_parent",
            "block::split-0",
            ":END:",
            ":PROPERTIES:",
            "#+ID:",
            "a:b",
            "kid:",
            "caf\u{e9}",
            too_long.as_str(),
        ] {
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
        assert_eq!(HeadlineDrawer.encode(""), "");
        assert_eq!(HeadlineDrawer.decode("\"\""), "\"\"");
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

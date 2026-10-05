//! The typed stream a format plugin emits, in one crate that both the wasm
//! guest writing it and the host reading it compile.
//!
//! A row's id is structured — unencoded path segments, then `::` parts — and
//! never a joined string, so a guest has no way to spell an id the host would
//! have to check. The host turns each [`LocalId`] into its stored reference in
//! one place, percent-encoding every segment and part.
//!
//! The wire is JSON Lines: line 1 is the [`Envelope`], every later line one
//! [`Line`].

use std::fmt;
use std::marker::PhantomData;

use serde::Deserialize;
use serde::Deserializer;
use serde::Serialize;
use serde::Serializer;
use serde::de::MapAccess;
use serde::de::Visitor;
use serde::ser::SerializeMap;
use serde_json::Value;

/// The stream version this crate reads and writes, stated by line 1.
pub const CONTRACT_VERSION: u32 = 2;

/// Why a [`LocalId`] or [`BlockKey`] cannot be built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IdError {
    /// A local id names no path at all.
    NoPath,
    /// A path segment is empty: the path had a leading, trailing or doubled
    /// `/`.
    EmptySegment { path: Vec<String> },
    /// A path segment holds a `/`, which only ever separates segments.
    SlashInSegment { segment: String },
    /// A `::` part is empty.
    EmptyPart { parts: Vec<String> },
    /// A block key has no part.
    NoParts,
}

impl fmt::Display for IdError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoPath => write!(f, "malformed local id: it names no path segment"),
            Self::EmptySegment { path } => {
                write!(f, "malformed local id: path {path:?} has an empty segment")
            }
            Self::SlashInSegment { segment } => write!(
                f,
                "malformed local id: path segment {segment:?} holds a '/', which only separates \
                 segments"
            ),
            Self::EmptyPart { parts } => {
                write!(
                    f,
                    "malformed local id: parts {parts:?} include an empty one"
                )
            }
            Self::NoParts => write!(f, "malformed block key: it has no part"),
        }
    }
}

impl std::error::Error for IdError {}

/// A row's id within its entity: a vault-relative path plus `::` parts, as
/// `Rezepte/Brot.cook` or `Rezepte/Brot.cook::iu::mehl-0`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "LocalIdWire")]
pub struct LocalId {
    path: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    parts: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalIdWire {
    path: Vec<String>,
    #[serde(default)]
    parts: Vec<String>,
}

impl TryFrom<LocalIdWire> for LocalId {
    type Error = IdError;

    fn try_from(wire: LocalIdWire) -> Result<Self, IdError> {
        if wire.path.is_empty() {
            return Err(IdError::NoPath);
        }
        if wire.path.iter().any(String::is_empty) {
            return Err(IdError::EmptySegment { path: wire.path });
        }
        if let Some(segment) = wire.path.iter().find(|s| s.contains('/')) {
            return Err(IdError::SlashInSegment {
                segment: segment.clone(),
            });
        }
        if wire.parts.iter().any(String::is_empty) {
            return Err(IdError::EmptyPart { parts: wire.parts });
        }
        Ok(Self {
            path: wire.path,
            parts: wire.parts,
        })
    }
}

impl LocalId {
    /// The id of the thing at `path`, split at `/`.
    pub fn from_path(path: &str) -> Result<Self, IdError> {
        Self::try_from(LocalIdWire {
            path: path.split('/').map(str::to_string).collect(),
            parts: Vec::new(),
        })
    }

    /// This id with `part` appended after a `::`.
    pub fn part(mut self, part: impl Into<String>) -> Result<Self, IdError> {
        self.parts.push(part.into());
        Self::try_from(LocalIdWire {
            path: self.path,
            parts: self.parts,
        })
    }

    /// The path segments, unencoded. Never empty; no segment is empty or holds
    /// a `/`.
    pub fn path_segments(&self) -> &[String] {
        &self.path
    }

    /// The `::` parts, unencoded. No part is empty.
    pub fn parts(&self) -> &[String] {
        &self.parts
    }
}

/// A block's id within its document, as the `::` parts that follow the
/// document's path: `["b", "0"]`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Vec<String>")]
pub struct BlockKey(Vec<String>);

impl TryFrom<Vec<String>> for BlockKey {
    type Error = IdError;

    fn try_from(parts: Vec<String>) -> Result<Self, IdError> {
        if parts.is_empty() {
            return Err(IdError::NoParts);
        }
        if parts.iter().any(String::is_empty) {
            return Err(IdError::EmptyPart { parts });
        }
        Ok(Self(parts))
    }
}

impl BlockKey {
    pub fn new<P: Into<String>>(parts: impl IntoIterator<Item = P>) -> Result<Self, IdError> {
        Self::try_from(parts.into_iter().map(Into::into).collect::<Vec<_>>())
    }

    /// Never empty; no part is empty.
    pub fn parts(&self) -> &[String] {
        &self.0
    }
}

/// A reference to a row of a declared type, as an ingredient use names its
/// recipe.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityRef {
    #[serde(rename = "type")]
    pub type_name: String,
    pub id: LocalId,
}

/// Named values in the order they were stated. A name stated twice is a
/// deserialization error rather than last-one-wins, which would read a row as
/// having said once what it said ambiguously.
#[derive(Debug, Clone, PartialEq)]
pub struct Fields<V>(Vec<(String, V)>);

impl<V> Default for Fields<V> {
    fn default() -> Self {
        Self(Vec::new())
    }
}

impl<V> Fields<V> {
    pub fn new() -> Self {
        Self::default()
    }

    /// Panics when `name` is already present: a producer stating a field twice
    /// is a bug in that producer.
    pub fn insert(&mut self, name: impl Into<String>, value: V) {
        let name = name.into();
        assert!(self.get(&name).is_none(), "field {name:?} stated twice");
        self.0.push((name, value));
    }

    pub fn get(&self, name: &str) -> Option<&V> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, v)| v)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.0.iter().map(|(n, v)| (n.as_str(), v))
    }
}

impl<V> IntoIterator for Fields<V> {
    type Item = (String, V);
    type IntoIter = std::vec::IntoIter<(String, V)>;

    fn into_iter(self) -> Self::IntoIter {
        self.0.into_iter()
    }
}

impl<V: Serialize> Serialize for Fields<V> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.0.len()))?;
        for (name, value) in &self.0 {
            map.serialize_entry(name, value)?;
        }
        map.end()
    }
}

impl<'de, V: Deserialize<'de>> Deserialize<'de> for Fields<V> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(FieldsVisitor(PhantomData))
    }
}

struct FieldsVisitor<V>(PhantomData<V>);

impl<'de, V: Deserialize<'de>> Visitor<'de> for FieldsVisitor<V> {
    type Value = Fields<V>;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str("an object of named fields")
    }

    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Fields<V>, A::Error> {
        let mut fields = Vec::new();
        while let Some((name, value)) = map.next_entry::<String, V>()? {
            if fields.iter().any(|(n, _): &(String, V)| *n == name) {
                return Err(serde::de::Error::custom(format!(
                    "field {name:?} is stated twice"
                )));
            }
            fields.push((name, value));
        }
        Ok(Fields(fields))
    }
}

/// Line 1: every scope the stream replaces, declared before any row. A scope
/// with no following row is how the last row of a type gets swept.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub holon_rows: u32,
    pub scopes: Vec<Scope>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    #[serde(rename = "type")]
    pub type_name: String,
    /// The column every row of the scope carries `owner` in.
    pub owner_column: String,
    pub owner: Owner,
}

/// What a scope is owned by: plain text such as a source path, or a row of
/// another type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Owner {
    Text(String),
    Ref(EntityRef),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Line {
    /// The file's own document. A stream carries exactly one.
    Document(DocumentRow),
    /// One child block of the document, in document order.
    Block(BlockRow),
    /// One row of a scope the envelope declares.
    Row(Row),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DocumentRow {
    pub title: String,
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub properties: Fields<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BlockRow {
    pub key: BlockKey,
    pub content: String,
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub properties: Fields<Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Row {
    #[serde(rename = "type")]
    pub type_name: String,
    pub id: LocalId,
    /// Columns that reference another row.
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub refs: Fields<EntityRef>,
    /// Every other column.
    #[serde(default, skip_serializing_if = "Fields::is_empty")]
    pub cells: Fields<Value>,
}

/// A whole plugin output: the envelope's scopes and the lines after it.
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    pub scopes: Vec<Scope>,
    pub lines: Vec<Line>,
}

/// A stream line that does not read as the contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamError {
    /// 1-based.
    pub line: usize,
    pub message: String,
}

impl fmt::Display for StreamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "line {}: {}", self.line, self.message)
    }
}

impl std::error::Error for StreamError {}

#[derive(Deserialize)]
struct VersionProbe {
    holon_rows: u32,
}

impl Stream {
    pub fn to_jsonl(&self) -> String {
        let envelope = Envelope {
            holon_rows: CONTRACT_VERSION,
            scopes: self.scopes.clone(),
        };
        let mut out = serde_json::to_string(&envelope).expect("an envelope always serializes");
        out.push('\n');
        for line in &self.lines {
            out.push_str(&serde_json::to_string(line).expect("a stream line always serializes"));
            out.push('\n');
        }
        out
    }

    pub fn from_jsonl(text: &str) -> Result<Self, StreamError> {
        let error = |line: usize, message: String| StreamError { line, message };
        let mut lines = text
            .split_inclusive('\n')
            .map(|line| line.trim_end_matches('\n'));

        let header = lines
            .next()
            .ok_or_else(|| error(1, "the stream is empty; line 1 must be the envelope".into()))?;
        let probe: VersionProbe =
            serde_json::from_str(header).map_err(|e| error(1, format!("not an envelope: {e}")))?;
        if probe.holon_rows != CONTRACT_VERSION {
            return Err(error(
                1,
                format!(
                    "declares holon_rows {}, and this build speaks {CONTRACT_VERSION}",
                    probe.holon_rows
                ),
            ));
        }
        let envelope: Envelope =
            serde_json::from_str(header).map_err(|e| error(1, format!("not an envelope: {e}")))?;

        let mut parsed = Vec::new();
        for (offset, line) in lines.enumerate() {
            let number = offset + 2;
            if line.is_empty() {
                return Err(error(
                    number,
                    "blank; every line after the envelope is one line of output".into(),
                ));
            }
            parsed.push(serde_json::from_str(line).map_err(|e| error(number, e.to_string()))?);
        }
        Ok(Self {
            scopes: envelope.scopes,
            lines: parsed,
        })
    }
}

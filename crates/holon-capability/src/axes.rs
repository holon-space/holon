//! The fidelity axes a capability profile declares.
//!
//! Increment 2b.1 carries axes 3 (`property_keys`) and 4 (`property_values`)
//! only. The other eight arrive in 2b.2; `deny_unknown_fields` means a yaml
//! naming one of them is a LOAD ERROR today rather than a silently ignored
//! section, so a profile can never claim more than the code checks.

use std::collections::BTreeSet;

use serde::Deserialize;
use serde::Serialize;

/// A property-key prefix the format OWNS: a key carrying it does not survive
/// as an ordinary property.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ReservedPrefix(String);

impl ReservedPrefix {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An exact property key, used both for the format's reserved list and for
/// naming the offending key in a [`crate::Violation`].
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PropertyKey(String);

impl PropertyKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Which key spellings the format can carry at all.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCharset {
    Any,
    /// A key containing whitespace is not a property at all.
    NoWhitespace,
    Identifier,
    KeywordNamespaced,
}

/// Whether the format preserves the authored spelling of a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyCase {
    Sensitive,
    FoldedUpper,
    FoldedLower,
}

/// What happens when two keys collide after `case` folding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Collision {
    LastWins,
    FirstWins,
    Error,
    MultiValued,
}

/// Whether an undeclared key is an error (logseq-db) or simply carried (org).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SchemaRequirement {
    /// Any key may be written without prior declaration.
    Open,
    /// A key the schema does not declare is refused.
    Declared,
}

/// Axis 3 — what the format can carry as a property KEY.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyKeysAxis {
    pub charset: KeyCharset,
    pub case: KeyCase,
    #[serde(default)]
    pub reserved_prefixes: Vec<ReservedPrefix>,
    #[serde(default)]
    pub reserved_keys: Vec<PropertyKey>,
    pub collision: Collision,
    pub schema_required: SchemaRequirement,
}

impl PropertyKeysAxis {
    /// Whether `key` carries a prefix the format ERASES.
    ///
    /// Distinct from [`Self::is_owned`] on purpose: a prefix reservation is a
    /// statement that the key does not come back, so its loss is honest and
    /// its SURVIVAL is the surprise.
    pub fn is_prefix_reserved(&self, key: &str) -> bool {
        self.reserved_prefixes
            .iter()
            .any(|p| key.starts_with(p.as_str()))
    }

    /// Whether `key` is one the format OWNS by exact spelling.
    ///
    /// An owned key is not an ordinary property and is not claimed to vanish
    /// — `ID` both survives and means something. What it round-trips THROUGH
    /// is the format's own machinery, so the ordinary-property law says
    /// nothing about it; axis 7 (`identity`) is what certifies it, and that
    /// axis arrives in 2b.2.
    pub fn is_owned(&self, key: &str) -> bool {
        self.reserved_keys.iter().any(|k| k.as_str() == key)
    }
}

/// The `Value` variants, as a value space a profile can name.
///
/// Mirrors `holon_pattern::Value` (`crates/holon-pattern/src/value.rs:21-37`).
/// A separate enum rather than `Value` itself because a profile names KINDS,
/// never inhabitants.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueKind {
    String,
    Integer,
    Float,
    Boolean,
    DateTime,
    Json,
    Array,
    Object,
    Null,
}

impl ValueKind {
    pub fn of(value: &holon_api::Value) -> Self {
        match value {
            holon_api::Value::String(_) => Self::String,
            holon_api::Value::Integer(_) => Self::Integer,
            holon_api::Value::Float(_) => Self::Float,
            holon_api::Value::Boolean(_) => Self::Boolean,
            holon_api::Value::DateTime(_) => Self::DateTime,
            holon_api::Value::Json(_) => Self::Json,
            holon_api::Value::Array(_) => Self::Array,
            holon_api::Value::Object(_) => Self::Object,
            holon_api::Value::Null => Self::Null,
        }
    }
}

/// Whether a particular inhabitant survives, vanishes, or is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Representability {
    Representable,
    Dropped,
    Error,
}

/// How the format carries more than one value under one key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MultiValue {
    None,
    Delimited {
        separator: String,
        semantics: MultiValueSemantics,
        scope: MultiValueScope,
    },
    NativeVector {
        semantics: MultiValueSemantics,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiValueSemantics {
    /// Order is semantic and must round-trip.
    List,
    /// Order is not semantic; the format may reorder.
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MultiValueScope {
    /// Every property splits on the separator.
    AllProperties,
    /// Only the format's own edge fields split; an ordinary property
    /// containing the separator stays one value.
    EdgeFieldsOnly,
}

/// How a property refers to another entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceValues {
    None,
    ByName,
    ById,
    VectorOfRefs,
}

/// Axis 4 — what the format can carry as a property VALUE.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PropertyValuesAxis {
    /// The `Value` kinds that round-trip preserving BOTH kind and inhabitant.
    /// A kind that survives only by being re-typed (org's integer coming back
    /// as `String`) does NOT belong here — see the org profile's rationale.
    pub types: BTreeSet<ValueKind>,
    pub empty_string: Representability,
    pub null: Representability,
    pub multi_value: MultiValue,
    pub reference_values: ReferenceValues,
}

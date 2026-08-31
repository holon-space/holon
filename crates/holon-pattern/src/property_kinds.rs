//! The per-key kind map that carries a property's KIND across the schemaless
//! JSON bag.
//!
//! JSON has no date kind and no opaque-document kind, so
//! [`Value::DateTime`] and [`Value::Json`] serialize into shapes that
//! [`Value::from_json_value`] reads back as [`Value::String`] and
//! [`Value::Object`]. This map records the kind for exactly those keys; every
//! other kind — [`Value::Null`] included — is evident from the JSON itself and
//! is deliberately ABSENT, so the common bag stores no map at all.

use std::collections::BTreeMap;
use std::collections::HashMap;

use serde::Deserialize;
use serde::Serialize;

use crate::Value;

/// A kind whose JSON form is indistinguishable from another kind's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AmbiguousKind {
    /// JSON string, indistinguishable from [`Value::String`].
    DateTime,
    /// JSON document, indistinguishable from [`Value::Object`] /
    /// [`Value::Array`] / a scalar.
    Json,
}

impl AmbiguousKind {
    /// The kind an entry is needed for, or `None` when the value's JSON form
    /// already names its kind.
    ///
    /// Every bag writer asks this — a writer that stores a value without
    /// consulting it leaves the kind map disagreeing with the bag.
    pub fn of(value: &Value) -> Option<Self> {
        match value {
            Value::DateTime(_) => Some(Self::DateTime),
            Value::Json(_) => Some(Self::Json),
            _ => None,
        }
    }

    /// The spelling stored in the `property_kinds` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::DateTime => "date_time",
            Self::Json => "json",
        }
    }
}

/// The kinds of a property bag's ambiguous keys.
///
/// Ordered so the stored column is byte-stable for the write leg's diff guard.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PropertyKinds(BTreeMap<String, AmbiguousKind>);

/// A stored kind map that disagrees with the bag it describes.
#[derive(Debug, thiserror::Error)]
pub enum PropertyKindsError {
    #[error("the property_kinds column must be a JSON object of key→kind, got {0:?}")]
    NotAKindMap(Value),
    #[error("property_kinds is not valid JSON ({source}): {raw:?}")]
    Malformed {
        raw: String,
        source: serde_json::Error,
    },
    #[error(
        "property_kinds says {key:?} is a date_time, but the stored value is not an RFC3339 \
         timestamp: {found:?}"
    )]
    NotADateTime { key: String, found: Value },
}

impl PropertyKinds {
    /// Derive the map a bag needs from its typed values.
    pub fn of<'a, I>(properties: I) -> Self
    where
        I: IntoIterator<Item = (&'a str, &'a Value)>,
    {
        Self(
            properties
                .into_iter()
                .filter_map(|(k, v)| AmbiguousKind::of(v).map(|kind| (k.to_string(), kind)))
                .collect(),
        )
    }

    /// The value for the `property_kinds` column, or `None` when no key needs
    /// an entry — which stores SQL NULL rather than an empty object.
    pub fn to_column(&self) -> Option<String> {
        if self.0.is_empty() {
            return None;
        }
        Some(serde_json::to_string(&self.0).expect("a BTreeMap of unit enums must serialize"))
    }

    /// Parse a stored `property_kinds` column. A missing column and a stored
    /// NULL both mean "no key carries a non-evident kind" — the reading a
    /// pre-NV-1 row deserves.
    pub fn parse_column(stored: Option<&Value>) -> Result<Self, PropertyKindsError> {
        let map = match stored {
            None | Some(Value::Null) => return Ok(Self::default()),
            Some(Value::String(raw)) => {
                serde_json::from_str(raw).map_err(|source| PropertyKindsError::Malformed {
                    raw: raw.clone(),
                    source,
                })?
            }
            // Turso hands a JSON TEXT column back as an Object on some read
            // paths, so both shapes reach here for the same stored bytes.
            Some(object @ Value::Object(_)) => {
                let json: serde_json::Value = object.clone().into();
                serde_json::from_value(json).map_err(|source| PropertyKindsError::Malformed {
                    raw: object_debug(object),
                    source,
                })?
            }
            Some(other) => return Err(PropertyKindsError::NotAKindMap(other.clone())),
        };
        Ok(Self(map))
    }

    /// Restore the recorded kinds over a bag already parsed by
    /// [`Value::from_json_value`].
    ///
    /// A key the map does not name keeps what JSON said. A key it names whose
    /// value cannot inhabit that kind is corruption and errors.
    pub fn retype(
        &self,
        bag: HashMap<String, Value>,
    ) -> Result<HashMap<String, Value>, PropertyKindsError> {
        bag.into_iter()
            .map(|(key, value)| {
                let restored = match self.0.get(&key) {
                    None => value,
                    Some(AmbiguousKind::DateTime) => {
                        let text =
                            value.as_string().filter(|s| is_rfc3339(s)).ok_or_else(|| {
                                PropertyKindsError::NotADateTime {
                                    key: key.clone(),
                                    found: value.clone(),
                                }
                            })?;
                        Value::DateTime(text.to_string())
                    }
                    // The Json variant carries a DOCUMENT, not its byte
                    // formatting: re-serializing yields the canonical spelling
                    // of the same JSON the author handed in.
                    Some(AmbiguousKind::Json) => {
                        let json: serde_json::Value = value.into();
                        Value::Json(
                            serde_json::to_string(&json)
                                .expect("a value that came from JSON must serialize"),
                        )
                    }
                };
                Ok((key, restored))
            })
            .collect()
    }

    /// The kinds after an update that names `written` and stores `newer`.
    ///
    /// Every named key is cleared before `newer` is laid down, so a key
    /// rewritten from a `DateTime` to a plain string — or removed outright —
    /// loses its entry instead of keeping one the bag no longer supports.
    pub fn merged_with<'a, W>(mut self, newer: Self, written: W) -> Self
    where
        W: IntoIterator<Item = &'a str>,
    {
        for key in written {
            self.0.remove(key);
        }
        self.0.extend(newer.0);
        self
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

fn is_rfc3339(text: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(text).is_ok()
}

fn object_debug(value: &Value) -> String {
    format!("{value:?}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bag(pairs: &[(&str, Value)]) -> HashMap<String, Value> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn only_ambiguous_kinds_get_an_entry() {
        let props = bag(&[
            ("when", Value::DateTime("2026-08-22T10:00:00Z".into())),
            ("doc", Value::Json(r#"{"a":1}"#.into())),
            ("plain", Value::String("x".into())),
            ("nothing", Value::Null),
            ("n", Value::Integer(1)),
        ]);
        let kinds = PropertyKinds::of(props.iter().map(|(k, v)| (k.as_str(), v)));
        assert_eq!(
            kinds.to_column().as_deref(),
            Some(r#"{"doc":"json","when":"date_time"}"#)
        );
    }

    #[test]
    fn an_unambiguous_bag_stores_no_column() {
        let props = bag(&[("plain", Value::String("x".into())), ("z", Value::Null)]);
        let kinds = PropertyKinds::of(props.iter().map(|(k, v)| (k.as_str(), v)));
        assert_eq!(kinds.to_column(), None);
    }

    #[test]
    fn a_missing_column_retypes_nothing() {
        let kinds = PropertyKinds::parse_column(None).expect("absent is the empty map");
        let stored = bag(&[("when", Value::String("2026-08-22T10:00:00Z".into()))]);
        assert_eq!(kinds.retype(stored.clone()).expect("no entries"), stored);
    }

    #[test]
    fn recorded_kinds_come_back_typed() {
        let sent = bag(&[
            ("when", Value::DateTime("2026-08-22T10:00:00Z".into())),
            ("doc", Value::Json(r#"{"a":1}"#.into())),
        ]);
        let kinds = PropertyKinds::of(sent.iter().map(|(k, v)| (k.as_str(), v)));
        let column = Value::String(kinds.to_column().expect("both keys are ambiguous"));

        // What the JSON bag alone reads back as.
        let stored = bag(&[
            ("when", Value::String("2026-08-22T10:00:00Z".into())),
            ("doc", Value::Object(bag(&[("a", Value::Integer(1))]))),
        ]);
        let restored = PropertyKinds::parse_column(Some(&column))
            .expect("the column round-trips")
            .retype(stored)
            .expect("both values inhabit their kind");
        assert_eq!(restored, sent);
    }

    #[test]
    fn a_kind_its_value_cannot_inhabit_errors() {
        let column = Value::String(r#"{"when":"date_time"}"#.to_string());
        let stored = bag(&[("when", Value::String("not a date".into()))]);
        let err = PropertyKinds::parse_column(Some(&column))
            .expect("the column parses")
            .retype(stored)
            .expect_err("a date_time entry over a non-date must fail loud");
        assert!(
            matches!(&err, PropertyKindsError::NotADateTime { key, .. } if key == "when"),
            "expected NotADateTime naming the key, got {err:?}"
        );
    }

    #[test]
    fn a_malformed_column_errors_rather_than_reading_as_empty() {
        let err = PropertyKinds::parse_column(Some(&Value::String("{not json".into())))
            .expect_err("malformed kinds must fail loud");
        assert!(matches!(err, PropertyKindsError::Malformed { .. }));
    }

    #[test]
    fn removals_drop_their_kind_entry() {
        let held = PropertyKinds::of([("when", &Value::DateTime("2026-08-22T10:00:00Z".into()))]);
        let merged = held.merged_with(PropertyKinds::default(), ["when"]);
        assert!(merged.is_empty());
    }

    #[test]
    fn rewriting_a_key_at_a_plain_kind_drops_its_entry() {
        let held = PropertyKinds::of([("when", &Value::DateTime("2026-08-22T10:00:00Z".into()))]);
        let merged = held.merged_with(
            PropertyKinds::of([("when", &Value::String("just text".into()))]),
            ["when"],
        );
        assert!(
            merged.is_empty(),
            "a stale date_time entry would re-type the new plain string"
        );
    }

    #[test]
    fn untouched_keys_keep_their_kinds() {
        let held = PropertyKinds::of([
            ("when", &Value::DateTime("2026-08-22T10:00:00Z".into())),
            ("doc", &Value::Json(r#"{"a":1}"#.into())),
        ]);
        let merged = held.merged_with(PropertyKinds::default(), ["doc"]);
        assert_eq!(
            merged.to_column().as_deref(),
            Some(r#"{"when":"date_time"}"#)
        );
    }
}

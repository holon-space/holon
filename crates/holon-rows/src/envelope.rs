//! Line 1 of a row stream: the scopes the following rows belong to, and the
//! kinds JSON cannot spell for itself.

use std::collections::BTreeMap;

use holon_api::AmbiguousKind;
use serde::Deserialize;
use serde::Serialize;

/// Bumped when a stream a previous version emitted would be MISREAD by this
/// one. A stream carrying any other number is refused rather than guessed at.
pub const CONTRACT_VERSION: u32 = 1;

/// One [`holon_core::file_format::TypedRowSet`]'s identity, declared before
/// its rows.
///
/// A scope with zero following rows is legal and load-bearing: it is how the
/// LAST row of a set gets swept, which inferring scopes from the rows present
/// would make unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScopeHeader {
    #[serde(rename = "type")]
    pub type_name: String,
    pub owner_column: String,
    pub owner_value: String,
    /// The columns whose kind their JSON form does not name — the same job
    /// the SQL leg's `property_kinds` column does, at column rather than key
    /// granularity because a scope types a column once for all its rows.
    ///
    /// Absent for the common scope, so a `jaq` filter writing plain JSON
    /// produces a legal stream without knowing this field exists.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub kinds: BTreeMap<String, AmbiguousKind>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope {
    pub holon_rows: u32,
    pub scopes: Vec<ScopeHeader>,
}

/// A row line: the scope it lands in, plus the row itself as a plain JSON
/// object so a `jaq` filter reads it without unwrapping anything.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RowLine {
    #[serde(rename = "type")]
    pub type_name: String,
    pub row: RowCells,
}

/// One row's cells, in the order the line states them.
///
/// Not a `serde_json::Map`: that keeps the LAST of two same-named cells, so a
/// row stating a column twice would be read as having stated it once, with a
/// value the producer never unambiguously gave.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RowCells(pub Vec<(String, serde_json::Value)>);

impl Serialize for RowCells {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_map(self.0.iter().map(|(k, v)| (k, v)))
    }
}

impl<'de> Deserialize<'de> for RowCells {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        deserializer.deserialize_map(RowCellsVisitor)
    }
}

struct RowCellsVisitor;

impl<'de> serde::de::Visitor<'de> for RowCellsVisitor {
    type Value = RowCells;

    fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        f.write_str("a row object whose columns are each stated once")
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<RowCells, A::Error> {
        let mut cells: Vec<(String, serde_json::Value)> = Vec::new();
        while let Some((column, value)) = map.next_entry::<String, serde_json::Value>()? {
            if let Some(first) = cells.iter().position(|(seen, _)| *seen == column) {
                return Err(serde::de::Error::custom(format!(
                    "column {column:?} is stated twice, at position {} and position {}",
                    first + 1,
                    cells.len() + 1
                )));
            }
            cells.push((column, value));
        }
        Ok(RowCells(cells))
    }
}

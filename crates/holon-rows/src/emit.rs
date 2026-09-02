//! `TypedRowSet` → JSON Lines.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use anyhow::Result;
use anyhow::bail;
use holon_api::AmbiguousKind;
use holon_api::Value;
use holon_core::file_format::TypedRowSet;

use crate::envelope::CONTRACT_VERSION;
use crate::envelope::Envelope;
use crate::envelope::RowCells;
use crate::envelope::RowLine;
use crate::envelope::ScopeHeader;

/// Serialize `sets` as an envelope line followed by one line per row.
///
/// Refuses rather than emits whenever a value has no faithful JSON form:
/// `serde_json` writes a non-finite float and an unparseable `Json` payload as
/// `null`, and a NULL nobody wrote is the one outcome a row stream must never
/// produce.
pub fn emit_row_sets(sets: &[TypedRowSet]) -> Result<String> {
    let mut seen_types = BTreeSet::new();
    let mut scopes = Vec::with_capacity(sets.len());
    for set in sets {
        if !seen_types.insert(set.type_name.as_str()) {
            bail!(
                "two scopes both declare type {:?}, so a row line naming it could not be routed \
                 to either",
                set.type_name
            );
        }
        scopes.push(ScopeHeader {
            type_name: set.type_name.clone(),
            owner_column: set.owner_column.clone(),
            owner_value: set.owner_value.clone(),
            kinds: kinds_of(set)?,
        });
    }

    let mut out = serde_json::to_string(&Envelope {
        holon_rows: CONTRACT_VERSION,
        scopes,
    })?;
    out.push('\n');

    for set in sets {
        for row in &set.rows {
            let line = RowLine {
                type_name: set.type_name.clone(),
                row: RowCells(
                    row.iter()
                        .map(|(column, value)| {
                            (column.to_string(), serde_json::Value::from(value.clone()))
                        })
                        .collect(),
                ),
            };
            out.push_str(&serde_json::to_string(&line)?);
            out.push('\n');
        }
    }

    Ok(out)
}

/// What one column's non-null values proved about its kind.
enum Witness {
    /// A kind JSON cannot spell, so the envelope must carry it.
    Ambiguous(AmbiguousKind),
    /// A value whose own JSON form names its kind.
    Plain,
}

impl Witness {
    fn describe(&self) -> &'static str {
        match self {
            Self::Ambiguous(kind) => kind.as_str(),
            Self::Plain => "a kind JSON already names",
        }
    }
}

/// The kind map for `set`, refusing every value that would reach the wire as
/// something other than itself.
fn kinds_of(set: &TypedRowSet) -> Result<BTreeMap<String, AmbiguousKind>> {
    let mut witnesses: BTreeMap<&str, Witness> = BTreeMap::new();
    let mut nulled: BTreeSet<&str> = BTreeSet::new();
    for row in &set.rows {
        for (column, value) in row {
            let witness = match value {
                Value::Null => {
                    nulled.insert(column.as_ref());
                    continue;
                }
                Value::Removed(_) => bail!(
                    "{}.{column} holds the removal marker, which is a write-leg instruction to \
                     DELETE a property rather than a row value",
                    set.type_name
                ),
                Value::Float(f) if !f.is_finite() => bail!(
                    "{}.{column} holds the non-finite float {f}, which JSON writes as `null` — a \
                     NULL nobody wrote",
                    set.type_name
                ),
                Value::Json(text) if serde_json::from_str::<serde_json::Value>(text).is_err() => {
                    bail!(
                        "{}.{column} is typed as a JSON document but holds {text:?}, which JSON \
                         writes as `null` — a NULL nobody wrote",
                        set.type_name
                    )
                }
                other => match AmbiguousKind::of(other) {
                    Some(kind) => Witness::Ambiguous(kind),
                    None => Witness::Plain,
                },
            };
            match witnesses.get(column.as_ref()) {
                None => {
                    witnesses.insert(column, witness);
                }
                Some(existing) if existing.describe() != witness.describe() => bail!(
                    "{}.{column} is {} in one row and {} in another, but the envelope types a \
                     column once for the whole scope",
                    set.type_name,
                    existing.describe(),
                    witness.describe()
                ),
                Some(_) => {}
            }
        }
    }

    let kinds: BTreeMap<String, AmbiguousKind> = witnesses
        .into_iter()
        .filter_map(|(column, witness)| match witness {
            Witness::Ambiguous(kind) => Some((column.to_string(), kind)),
            Witness::Plain => None,
        })
        .collect();

    // A JSON document and a NULL share the wire form `null`, and the kind map
    // types a column once for the whole scope — so a column stating both could
    // only be read back by guessing which row meant which.
    for column in kinds
        .iter()
        .filter(|(_, kind)| matches!(kind, AmbiguousKind::Json))
        .map(|(column, _)| column)
    {
        if nulled.contains(column.as_str()) {
            bail!(
                "{}.{column} holds a JSON document in one row and a NULL in another, and both \
                 reach the wire as `null`",
                set.type_name
            );
        }
    }

    Ok(kinds)
}

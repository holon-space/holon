//! JSON Lines → `TypedRowSet`.

use std::collections::BTreeMap;

use anyhow::Context as _;
use anyhow::Result;
use anyhow::bail;
use holon_api::AmbiguousKind;
use holon_api::StorageEntity;
use holon_api::Value;
use holon_core::file_format::TypedRowSet;

use crate::envelope::CONTRACT_VERSION;
use crate::envelope::Envelope;
use crate::envelope::RowLine;

/// Read a row stream back into the sets it was emitted from, in envelope
/// order.
///
/// A malformed line is an `Err` naming the line, never a skipped row: a
/// producer that emitted one bad row of a set has produced a set that would
/// sweep the rows it failed to re-state.
pub fn parse_row_sets(input: &str) -> Result<Vec<TypedRowSet>> {
    let mut lines = input
        .split_inclusive('\n')
        .map(|line| line.trim_end_matches('\n'));

    let header = lines
        .next()
        .context("the row stream is empty; line 1 must be the envelope")?;
    let envelope: Envelope =
        serde_json::from_str(header).context("line 1 is not a row-stream envelope")?;
    if envelope.holon_rows != CONTRACT_VERSION {
        bail!(
            "line 1 declares holon_rows {}, and this build speaks {CONTRACT_VERSION}",
            envelope.holon_rows
        );
    }

    let mut kinds: Vec<&BTreeMap<String, AmbiguousKind>> = Vec::new();
    let mut sets: Vec<TypedRowSet> = Vec::new();
    for scope in &envelope.scopes {
        if sets.iter().any(|s| s.type_name == scope.type_name) {
            bail!(
                "line 1 declares type {:?} twice, so a row line naming it could not be routed to \
                 either scope",
                scope.type_name
            );
        }
        kinds.push(&scope.kinds);
        sets.push(TypedRowSet {
            type_name: scope.type_name.clone(),
            owner_column: scope.owner_column.clone(),
            owner_value: scope.owner_value.clone(),
            rows: Vec::new(),
        });
    }

    for (offset, line) in lines.enumerate() {
        let number = offset + 2;
        if line.is_empty() {
            bail!("line {number} is blank; every line after the envelope must carry one row");
        }
        let parsed: RowLine =
            serde_json::from_str(line).with_context(|| format!("line {number} is not a row"))?;
        let index = sets
            .iter()
            .position(|set| set.type_name == parsed.type_name)
            .with_context(|| {
                format!(
                    "line {number} carries type {:?}, which line 1 does not declare",
                    parsed.type_name
                )
            })?;

        let mut row = StorageEntity::new();
        for (column, json) in parsed.row.0 {
            let value = restore(&column, json, kinds[index])
                .with_context(|| format!("line {number}, column {column:?}"))?;
            row.insert(column.into(), value);
        }
        sets[index].rows.push(row);
    }

    Ok(sets)
}

/// The typed value one JSON cell stands for.
///
/// The twin of `PropertyKinds::retype`, which cannot serve here because it
/// reads a NULL as a kind violation: a row column is legitimately null, and
/// the kind describes what the column holds when it holds anything.
///
/// A `Json` cell comes back in its canonical spelling — the variant carries a
/// document, not its byte formatting, the same contract the SQL leg gets.
fn restore(
    column: &str,
    json: serde_json::Value,
    kinds: &BTreeMap<String, AmbiguousKind>,
) -> Result<Value> {
    match kinds.get(column) {
        // The kind decides before nullness does: `null` is a JSON document like
        // any other, and a column the envelope types as `json` holds documents.
        // The emitter refuses a scope that states both, so this cell is one or
        // the other, never a guess.
        Some(AmbiguousKind::Json) => Ok(Value::Json(serde_json::to_string(&json)?)),
        _ if json.is_null() => Ok(Value::Null),
        None => Ok(Value::from_json_value(json)),
        Some(AmbiguousKind::DateTime) => {
            let text = json.as_str().filter(|t| is_rfc3339(t)).with_context(|| {
                format!("declared a date_time, but {json} is not an RFC3339 timestamp")
            })?;
            Ok(Value::DateTime(text.to_string()))
        }
    }
}

fn is_rfc3339(text: &str) -> bool {
    chrono::DateTime::parse_from_rfc3339(text).is_ok()
}

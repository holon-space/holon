//! Widget specification types for backend-driven UI
//!
//! WidgetSpec is the unified return type for all rendered widgets.
//! The backend executes queries and returns both the render spec and data.
//! The frontend just renders using RenderInterpreter.

use std::collections::HashMap;

use crate::streaming::Change;
use crate::Value;

/// A single row of query result data (may or may not be enriched).
pub type DataRow = HashMap<String, Value>;

/// A row that has been through the enrichment pipeline (`flatten_properties` +
/// computed fields from entity profile resolution).
///
/// **Parse, don't validate**: The only way to obtain an `EnrichedRow` is through
/// the enrichment pipeline — there is no public constructor.  This makes it a
/// compile error to feed raw storage data into the reactive pipeline.
///
/// `Deref<Target = HashMap>` lets read-only code (`.get("task_state")`, etc.)
/// work unchanged.
#[derive(Debug, Clone, PartialEq)]
pub struct EnrichedRow(HashMap<String, Value>);

impl std::ops::Deref for EnrichedRow {
    type Target = HashMap<String, Value>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for EnrichedRow {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl EnrichedRow {
    /// Enrich a raw storage row: flatten `properties` JSON to top-level keys
    /// and inject caller-provided computed fields.
    ///
    /// This is the **only** way to create an `EnrichedRow`.  The `computed_fields`
    /// closure receives the flattened row and returns additional key-value pairs
    /// (typically from entity profile resolution).
    pub fn from_raw(
        data: HashMap<String, Value>,
        computed_fields: impl FnOnce(&HashMap<String, Value>) -> HashMap<String, Value>,
    ) -> Self {
        let mut row = Self::flatten_properties(data);
        for (key, value) in computed_fields(&row) {
            row.insert(key, value);
        }
        Self(row)
    }

    /// Convert back to a plain `DataRow` when crossing into code that hasn't
    /// been migrated to `EnrichedRow` yet.  Prefer removing these call sites
    /// over adding new ones.
    pub fn into_inner(self) -> HashMap<String, Value> {
        self.0
    }

    /// Promote fields from the `properties` JSON object to top-level row keys.
    fn flatten_properties(mut data: HashMap<String, Value>) -> HashMap<String, Value> {
        if let Some(Value::Object(props)) = data.get("properties") {
            for (key, value) in props.clone() {
                if !data.contains_key(&key) {
                    data.insert(key, value);
                }
            }
        }
        data
    }
}

/// Keyed collection of data rows with CDC change support.
///
/// Replaces the pattern of maintaining a `Vec<DataRow>` and doing linear scans
/// to apply Created/Updated/Deleted/FieldsChanged events. Uses a HashMap keyed
/// by the "id" column for O(1) lookups.
#[derive(Debug, Clone)]
pub struct DataRowAccumulator {
    rows: HashMap<String, DataRow>,
}

impl DataRowAccumulator {
    pub fn new() -> Self {
        Self {
            rows: HashMap::new(),
        }
    }

    pub fn from_rows(rows: Vec<DataRow>) -> Self {
        let mut map = HashMap::with_capacity(rows.len());
        for row in rows {
            if let Some(id) = row.get("id").and_then(|v| v.as_string()) {
                map.insert(id.to_string(), row);
            }
        }
        Self { rows: map }
    }

    pub fn apply_change(&mut self, change: Change<DataRow>) {
        match change {
            Change::Created { data, .. } => {
                if let Some(id) = data.get("id").and_then(|v| v.as_string()) {
                    self.rows.insert(id.to_string(), data);
                }
            }
            Change::Updated { ref id, data, .. } => {
                self.rows.insert(id.clone(), data);
            }
            Change::Deleted { ref id, .. } => {
                self.rows.remove(id);
            }
            Change::FieldsChanged {
                ref entity_id,
                ref fields,
                ..
            } => {
                if let Some(row) = self.rows.get_mut(entity_id) {
                    for (name, _old, new) in fields {
                        row.insert(name.clone(), new.clone());
                    }
                }
            }
        }
    }

    pub fn apply_batch(&mut self, changes: impl IntoIterator<Item = Change<DataRow>>) {
        for change in changes {
            self.apply_change(change);
        }
    }

    /// Export as Vec<DataRow> for interpretation.
    pub fn to_vec(&self) -> Vec<DataRow> {
        self.rows.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
}

impl Default for DataRowAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::streaming::ChangeOrigin;

    fn origin() -> ChangeOrigin {
        ChangeOrigin::Local {
            operation_id: None,
            trace_id: None,
        }
    }

    fn row(id: &str, content: &str) -> DataRow {
        HashMap::from([
            ("id".into(), Value::String(id.into())),
            ("content".into(), Value::String(content.into())),
        ])
    }

    #[test]
    fn accumulator_from_rows_and_to_vec() {
        let acc = DataRowAccumulator::from_rows(vec![row("a", "hello"), row("b", "world")]);
        assert_eq!(acc.len(), 2);
        let v = acc.to_vec();
        assert_eq!(v.len(), 2);
    }

    #[test]
    fn accumulator_apply_created() {
        let mut acc = DataRowAccumulator::new();
        acc.apply_change(Change::Created {
            data: row("x", "new"),
            origin: origin(),
        });
        assert_eq!(acc.len(), 1);
        let v = acc.to_vec();
        assert_eq!(v[0].get("content").unwrap().as_string().unwrap(), "new");
    }

    #[test]
    fn accumulator_apply_updated() {
        let mut acc = DataRowAccumulator::from_rows(vec![row("a", "old")]);
        acc.apply_change(Change::Updated {
            id: "a".into(),
            data: row("a", "updated"),
            origin: origin(),
        });
        assert_eq!(acc.len(), 1);
        let v = acc.to_vec();
        assert_eq!(v[0].get("content").unwrap().as_string().unwrap(), "updated");
    }

    #[test]
    fn accumulator_apply_deleted() {
        let mut acc = DataRowAccumulator::from_rows(vec![row("a", "bye")]);
        acc.apply_change(Change::Deleted {
            id: "a".into(),
            origin: origin(),
        });
        assert!(acc.is_empty());
    }

    #[test]
    fn accumulator_apply_fields_changed() {
        let mut acc = DataRowAccumulator::from_rows(vec![row("a", "old")]);
        acc.apply_change(Change::FieldsChanged {
            entity_id: "a".into(),
            fields: vec![(
                "content".into(),
                Value::String("old".into()),
                Value::String("patched".into()),
            )],
            origin: origin(),
        });
        let v = acc.to_vec();
        assert_eq!(v[0].get("content").unwrap().as_string().unwrap(), "patched");
    }

    #[test]
    fn accumulator_apply_batch() {
        let mut acc = DataRowAccumulator::new();
        acc.apply_batch([
            Change::Created {
                data: row("a", "first"),
                origin: origin(),
            },
            Change::Created {
                data: row("b", "second"),
                origin: origin(),
            },
            Change::Deleted {
                id: "a".into(),
                origin: origin(),
            },
        ]);
        assert_eq!(acc.len(), 1);
        assert!(acc
            .to_vec()
            .iter()
            .any(|r| { r.get("id").unwrap().as_string().unwrap() == "b" }));
    }
}

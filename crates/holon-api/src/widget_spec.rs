//! Widget specification types for backend-driven UI
//!
//! WidgetSpec is the unified return type for all rendered widgets.
//! The backend executes queries and returns both the render spec and data.
//! The frontend just renders using RenderInterpreter.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::render_types::RenderExpr;
use crate::streaming::Change;
use crate::Value;

/// A single row of query result data.
pub type DataRow = HashMap<String, Value>;

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

/// Specification for rendering a widget
///
/// WidgetSpec is the unified type returned by query_and_watch and initial_widget.
/// It contains everything the frontend needs to render:
/// - data: The query result data (rows)
/// - actions: Available actions (may be empty for non-root widgets)
///
/// The change stream is returned separately (not in this struct) since
/// streams are not serializable for FFI.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WidgetSpec {
    /// How to render this widget (required — backend always provides one)
    pub render_expr: RenderExpr,

    /// Query result rows.
    pub data: Vec<DataRow>,

    /// Available actions (global actions for root, contextual for others)
    pub actions: Vec<ActionSpec>,
}

impl WidgetSpec {
    /// Create a new WidgetSpec from raw query result rows.
    pub fn from_rows(data: Vec<DataRow>) -> Self {
        Self {
            render_expr: RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: Vec::new(),
            },
            data,
            actions: Vec::new(),
        }
    }

    /// Add an action
    pub fn with_action(mut self, action: ActionSpec) -> Self {
        self.actions.push(action);
        self
    }

    /// Set actions
    pub fn with_actions(mut self, actions: Vec<ActionSpec>) -> Self {
        self.actions = actions;
        self
    }
}

/// Specification for a global action
///
/// Actions are operations that can be triggered from anywhere in the app,
/// like sync, settings, or undo/redo.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSpec {
    /// Unique identifier
    pub id: String,

    /// Human-readable display name
    pub display_name: String,

    /// Optional icon name (e.g., "sync", "settings")
    pub icon: Option<String>,

    /// The operation to execute when this action is triggered
    pub operation: ActionOperation,
}

/// Operation to execute for an action
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionOperation {
    /// Entity name (e.g., "*" for wildcard operations)
    pub entity_name: String,

    /// Operation name (e.g., "sync_from_remote")
    pub op_name: String,

    /// Default parameters for this operation
    pub params: HashMap<String, Value>,
}

impl ActionSpec {
    /// Create a new action specification
    pub fn new(
        id: impl Into<String>,
        display_name: impl Into<String>,
        entity_name: impl Into<String>,
        op_name: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            display_name: display_name.into(),
            icon: None,
            operation: ActionOperation {
                entity_name: entity_name.into(),
                op_name: op_name.into(),
                params: HashMap::new(),
            },
        }
    }

    /// Set the icon
    pub fn with_icon(mut self, icon: impl Into<String>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    /// Add a parameter
    pub fn with_param(mut self, key: impl Into<String>, value: Value) -> Self {
        self.operation.params.insert(key.into(), value);
        self
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
    fn test_widget_spec_builder() {
        let spec = WidgetSpec::from_rows(vec![]).with_action(
            ActionSpec::new("sync", "Sync", "*", "sync_from_remote").with_icon("sync"),
        );

        assert_eq!(spec.actions.len(), 1);
        assert!(spec.data.is_empty());
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

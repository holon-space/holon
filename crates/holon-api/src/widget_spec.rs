//! Widget specification types for backend-driven UI
//!
//! WidgetSpec is the unified return type for all rendered widgets.
//! The backend executes queries and returns both the render spec and data.
//! The frontend just renders using RenderInterpreter.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use crate::render_types::{RenderExpr, RowProfile};
use crate::Value;

/// A row of query result data with its optional resolved EntityProfile.
///
/// flutter_rust_bridge:non_opaque
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResolvedRow {
    /// The row data from the query result
    pub data: HashMap<String, Value>,

    /// The resolved profile for this row, if an EntityProfile matched.
    /// `None` when no EntityProfile is defined for this row's entity.
    pub profile: Option<RowProfile>,
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

    /// Query result rows, each carrying its data and optional resolved profile.
    pub data: Vec<ResolvedRow>,

    /// Available actions (global actions for root, contextual for others)
    pub actions: Vec<ActionSpec>,
}

impl WidgetSpec {
    /// Create a new WidgetSpec from raw query result rows (no profiles).
    pub fn from_rows(data: Vec<HashMap<String, Value>>) -> Self {
        Self {
            render_expr: RenderExpr::FunctionCall {
                name: "table".to_string(),
                args: Vec::new(),
                operations: Vec::new(),
            },
            data: data
                .into_iter()
                .map(|d| ResolvedRow {
                    data: d,
                    profile: None,
                })
                .collect(),
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

    #[test]
    fn test_widget_spec_builder() {
        let spec = WidgetSpec::from_rows(vec![]).with_action(
            ActionSpec::new("sync", "Sync", "*", "sync_from_remote").with_icon("sync"),
        );

        assert_eq!(spec.actions.len(), 1);
        assert!(spec.data.is_empty());
    }
}

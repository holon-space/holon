//! Widget state tracking for Cucumber integration tests
//!
//! Provides a model to track UI state by applying CDC events to initial data rows.
//! Enables text-based assertions for realistic end-to-end testing.

use std::collections::HashMap;

use holon::api::{ChangeData, RowChange};
use holon_api::Value;
use holon_api::widget_spec::DataRow;
use indexmap::IndexMap;

/// Apply a CDC event to a Vec-based row collection.
///
/// This is the shared implementation used by both PBT tests (directly) and
/// Cucumber tests (through WidgetStateModel). It handles all ChangeData variants:
/// - Created: appends the new row
/// - Updated: replaces all fields in the matching row
/// - FieldsChanged: updates specific fields in the matching row
/// - Deleted: removes the row with matching entity ID
///
/// Rows are matched by their "id" field.
pub fn apply_cdc_event_to_vec(rows: &mut Vec<HashMap<String, Value>>, event: &RowChange) {
    match &event.change {
        ChangeData::Created { data, .. } => {
            rows.push(data.clone());
        }
        ChangeData::Updated { data, .. } => {
            if let Some(entity_id) = data.get("id").and_then(|v| v.as_string())
                && let Some(row) = rows.iter_mut().find(|r| {
                    r.get("id")
                        .and_then(|v| v.as_string())
                        .map(|s| s == entity_id)
                        .unwrap_or(false)
                })
            {
                for (k, v) in data {
                    row.insert(k.clone(), v.clone());
                }
            }
        }
        ChangeData::FieldsChanged {
            entity_id, fields, ..
        } => {
            if let Some(row) = rows.iter_mut().find(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s == *entity_id)
                    .unwrap_or(false)
            }) {
                for (field_name, _old_value, new_value) in fields {
                    row.insert(field_name.clone(), new_value.clone());
                }
            }
        }
        ChangeData::Deleted { id: rowid, .. } => {
            rows.retain(|r| {
                r.get("id")
                    .and_then(|v| v.as_string())
                    .map(|s| s != *rowid)
                    .unwrap_or(true)
            });
        }
    }
}

/// Widget locator for targeting specific widgets in assertions.
///
/// Designed for extensibility - "column 1" is just one locator type.
/// Future expansion could include path-based selectors like "main-view > list > item 3".
#[derive(Debug, Clone)]
pub enum WidgetLocator {
    /// Match by column index (1-based): "column 1", "column 2"
    Column(usize),
    /// Match by view ID: "sidebar", "main-view"
    ViewId(String),
    /// Match all widgets (for global assertions)
    All,
}

impl WidgetLocator {
    /// Parse a widget locator from a Gherkin step string.
    ///
    /// Supports:
    /// - "column 1", "column 2" → Column(n)
    /// - "sidebar", "main-view" → ViewId(s)
    /// - "all" → All
    pub fn parse(s: &str) -> Self {
        if s == "all" {
            return WidgetLocator::All;
        }
        if let Some(n_str) = s.strip_prefix("column ")
            && let Ok(n) = n_str.parse::<usize>()
        {
            return WidgetLocator::Column(n);
        }
        WidgetLocator::ViewId(s.to_string())
    }
}

/// Tracks the current state of a widget by applying CDC events to initial data.
///
/// Maintains an ordered collection of rows (preserving query result order) and
/// provides text extraction for assertions.
pub struct WidgetStateModel {
    /// Current data rows keyed by entity ID, preserving insertion order
    rows: IndexMap<String, HashMap<String, Value>>,
}

impl WidgetStateModel {
    /// Create a new WidgetStateModel from initial data rows.
    pub fn from_data(data: &[DataRow]) -> Self {
        let mut rows = IndexMap::new();
        for row in data {
            if let Some(id) = row.get("id").and_then(|v| v.as_string()) {
                rows.insert(id.to_string(), row.clone());
            }
        }
        Self { rows }
    }

    /// Apply a CDC change event to update the state.
    pub fn apply_change(&mut self, change: &RowChange) {
        match &change.change {
            ChangeData::Created { data, .. } => {
                if let Some(id) = data.get("id").and_then(|v| v.as_string()) {
                    self.rows.insert(id.to_string(), data.clone());
                }
            }
            ChangeData::Updated { data, .. } => {
                if let Some(id) = data.get("id").and_then(|v| v.as_string()) {
                    self.rows.insert(id.to_string(), data.clone());
                }
            }
            ChangeData::Deleted { id, .. } => {
                self.rows.shift_remove(id);
            }
            ChangeData::FieldsChanged {
                entity_id, fields, ..
            } => {
                if let Some(row) = self.rows.get_mut(entity_id) {
                    for (field, _old, new) in fields {
                        row.insert(field.clone(), new.clone());
                    }
                }
            }
        }
    }

    /// Extract text content for widgets matching the given locator.
    pub fn extract_text(&self, locator: &WidgetLocator) -> String {
        match locator {
            WidgetLocator::All => self.rows_to_text(self.rows.values().collect()),
            WidgetLocator::Column(n) => self.extract_column_text(*n),
            WidgetLocator::ViewId(id) => self.extract_view_text(id),
        }
    }

    /// Check if widgets matching the locator contain the expected text.
    pub fn contains_text(&self, locator: &WidgetLocator, expected: &str) -> bool {
        self.extract_text(locator).contains(expected)
    }

    /// Get the number of views (columns).
    /// TODO: render spec was removed; always returns 1 now
    pub fn view_count(&self) -> usize {
        1
    }

    /// Get all view names.
    /// TODO: render spec was removed; always empty now
    pub fn view_names(&self) -> Vec<String> {
        vec![]
    }

    /// Get all current row IDs (for debugging).
    pub fn row_ids(&self) -> Vec<String> {
        self.rows.keys().cloned().collect()
    }

    /// Get the current row count.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    // TODO: render spec was removed; column/view filtering is no longer available
    fn extract_column_text(&self, _column: usize) -> String {
        self.rows_to_text(self.rows.values().collect())
    }

    fn extract_view_text(&self, _view_id: &str) -> String {
        self.rows_to_text(self.rows.values().collect())
    }

    fn rows_to_text(&self, rows: Vec<&HashMap<String, Value>>) -> String {
        rows.iter()
            .flat_map(|row| {
                row.values()
                    .filter_map(|v| v.as_string())
                    .map(|s| s.to_string())
                    .collect::<Vec<_>>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl std::fmt::Debug for WidgetStateModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WidgetStateModel")
            .field("row_count", &self.rows.len())
            .field("row_ids", &self.row_ids())
            .field("views", &self.view_names())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_row(id: &str, content: &str) -> HashMap<String, Value> {
        let mut row = HashMap::new();
        row.insert("id".to_string(), Value::String(id.to_string()));
        row.insert("content".to_string(), Value::String(content.to_string()));
        row
    }

    #[test]
    fn test_locator_parse() {
        assert!(matches!(
            WidgetLocator::parse("column 1"),
            WidgetLocator::Column(1)
        ));
        assert!(matches!(
            WidgetLocator::parse("column 2"),
            WidgetLocator::Column(2)
        ));
        assert!(matches!(WidgetLocator::parse("all"), WidgetLocator::All));
        assert!(
            matches!(WidgetLocator::parse("sidebar"), WidgetLocator::ViewId(s) if s == "sidebar")
        );
    }

    #[test]
    fn test_from_data() {
        let rows = vec![make_row("block-1", "Hello"), make_row("block-2", "World")];
        let state = WidgetStateModel::from_data(&rows);

        assert_eq!(state.row_count(), 2);
        assert!(state.contains_text(&WidgetLocator::All, "Hello"));
        assert!(state.contains_text(&WidgetLocator::All, "World"));
    }

    #[test]
    fn test_extract_text() {
        let rows = vec![make_row("block-1", "Hello"), make_row("block-2", "World")];
        let state = WidgetStateModel::from_data(&rows);

        let text = state.extract_text(&WidgetLocator::All);
        assert!(text.contains("Hello"));
        assert!(text.contains("World"));
    }
}

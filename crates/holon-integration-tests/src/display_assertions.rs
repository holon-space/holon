use std::collections::HashMap;
use std::fmt::Write;

use holon_api::Value;
use holon_frontend::ViewModel;
use holon_frontend::view_model::NodeKind;

/// A single difference found between two ViewModel trees.
#[derive(Debug, Clone)]
pub struct TreeDiff {
    pub path: String,
    pub kind: DiffKind,
}

#[derive(Debug, Clone)]
pub enum DiffKind {
    WidgetMismatch { actual: String, expected: String },
    ValueMismatch { actual: String, expected: String },
    ChildCountMismatch { actual: usize, expected: usize },
    OnlyInActual,
    OnlyInExpected,
}

impl std::fmt::Display for TreeDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at {}: ", self.path)?;
        match &self.kind {
            DiffKind::WidgetMismatch { actual, expected } => {
                write!(f, "widget \"{actual}\" vs \"{expected}\"")
            }
            DiffKind::ValueMismatch { actual, expected } => {
                write!(f, "value {actual} vs {expected}")
            }
            DiffKind::ChildCountMismatch { actual, expected } => {
                write!(f, "{actual} children vs {expected}")
            }
            DiffKind::OnlyInActual => write!(f, "only in actual"),
            DiffKind::OnlyInExpected => write!(f, "only in expected"),
        }
    }
}

/// Recursively diff two ViewModel trees. Returns all differences found.
pub fn tree_diff(actual: &ViewModel, expected: &ViewModel) -> Vec<TreeDiff> {
    let mut diffs = Vec::new();
    diff_recursive(actual, expected, &mut String::from("root"), &mut diffs);
    diffs
}

fn diff_recursive(
    actual: &ViewModel,
    expected: &ViewModel,
    path: &mut String,
    diffs: &mut Vec<TreeDiff>,
) {
    // Compare widget names
    let aw = actual.widget_name().unwrap_or("(none)");
    let ew = expected.widget_name().unwrap_or("(none)");
    if aw != ew {
        diffs.push(TreeDiff {
            path: path.clone(),
            kind: DiffKind::WidgetMismatch {
                actual: aw.to_string(),
                expected: ew.to_string(),
            },
        });
        return;
    }

    // Compare children
    let ac = actual.children();
    let ec = expected.children();
    diff_children(ac, ec, path, diffs);
}

fn diff_children(
    actual: &[ViewModel],
    expected: &[ViewModel],
    path: &mut String,
    diffs: &mut Vec<TreeDiff>,
) {
    if actual.len() != expected.len() {
        diffs.push(TreeDiff {
            path: path.clone(),
            kind: DiffKind::ChildCountMismatch {
                actual: actual.len(),
                expected: expected.len(),
            },
        });
    }
    let min = actual.len().min(expected.len());
    for i in 0..min {
        let prev_len = path.len();
        write!(path, "/[{i}]").unwrap();
        diff_recursive(&actual[i], &expected[i], path, diffs);
        path.truncate(prev_len);
    }
    for i in min..actual.len() {
        diffs.push(TreeDiff {
            path: format!("{path}/[{i}]"),
            kind: DiffKind::OnlyInActual,
        });
    }
    for i in min..expected.len() {
        diffs.push(TreeDiff {
            path: format!("{path}/[{i}]"),
            kind: DiffKind::OnlyInExpected,
        });
    }
}

/// Check whether actual entity IDs are an ordered subset of expected IDs.
#[derive(Debug)]
pub struct OrderedSubsetResult {
    pub is_subset: bool,
    pub missing_from_expected: Vec<String>,
    pub out_of_order: Vec<(String, String)>,
}

pub fn is_ordered_subset(actual_ids: &[String], expected_ids: &[String]) -> OrderedSubsetResult {
    let mut missing_from_expected = Vec::new();
    let mut out_of_order = Vec::new();

    // Track which expected indices have been consumed so duplicate values
    // are matched one-to-one instead of all hitting the first occurrence.
    let mut consumed = vec![false; expected_ids.len()];
    let mut last_expected_idx: Option<usize> = None;

    for actual_id in actual_ids {
        // Find the first unconsumed match in expected_ids
        let found = expected_ids
            .iter()
            .enumerate()
            .position(|(i, e)| !consumed[i] && e == actual_id);

        match found {
            None => missing_from_expected.push(actual_id.clone()),
            Some(idx) => {
                consumed[idx] = true;
                if let Some(prev) = last_expected_idx {
                    if idx <= prev {
                        out_of_order.push((actual_id.clone(), expected_ids[prev].clone()));
                    }
                }
                last_expected_idx = Some(idx);
            }
        }
    }

    let is_subset = missing_from_expected.is_empty() && out_of_order.is_empty();
    OrderedSubsetResult {
        is_subset,
        missing_from_expected,
        out_of_order,
    }
}

/// Assert two ViewModel trees match structurally, with a nice error message on failure.
pub fn assert_display_trees_match(actual: &ViewModel, expected: &ViewModel, message: &str) {
    let diffs = tree_diff(actual, expected);
    assert!(
        diffs.is_empty(),
        "{message}\n\nFound {} difference(s):\n{}\n\n--- actual ---\n{}\n--- expected ---\n{}",
        diffs.len(),
        diffs
            .iter()
            .map(|d| format!("  {d}"))
            .collect::<Vec<_>>()
            .join("\n"),
        actual.pretty_print(0),
        expected.pretty_print(0),
    );
}

/// Extract per-row entity data from a rendered ViewModel tree.
///
/// Walks the collection children and extracts data based on node type:
/// - `TableRow { data }` → full row data
/// - `BlockRef { block_id }` → `{"id": block_id}`
/// - Wrapper nodes (Focusable, Selectable, etc.) → unwrap and recurse
///
/// This is the "decompiler" — inverse of the shadow interpreter's rendering.
/// It extracts whatever data is structurally present, regardless of template.
pub fn extract_rendered_rows(tree: &ViewModel) -> Vec<HashMap<String, Value>> {
    tree.children()
        .iter()
        .filter_map(|child| extract_item_data(child))
        .collect()
}

fn extract_item_data(node: &ViewModel) -> Option<HashMap<String, Value>> {
    match &node.kind {
        NodeKind::TableRow { data } => Some(data.clone()),
        NodeKind::BlockRef { block_id, .. } => Some(HashMap::from([(
            "id".to_string(),
            Value::String(block_id.clone()),
        )])),
        // Wrapper nodes: unwrap and try the inner child
        NodeKind::Focusable { child }
        | NodeKind::Selectable { child }
        | NodeKind::Draggable { child }
        | NodeKind::PieMenu { child, .. } => extract_item_data(child),
        // Layout/container with children: try extracting from first leaf-like descendant
        NodeKind::Row { children, .. } | NodeKind::Col { children } => {
            extract_from_children(&children.items)
        }
        _ => None,
    }
}

fn extract_from_children(children: &[ViewModel]) -> Option<HashMap<String, Value>> {
    // Walk children looking for data-bearing nodes
    let mut data = HashMap::new();
    for child in children {
        match &child.kind {
            NodeKind::Text { content, .. } => {
                // A text node rendering a column value
                data.insert("content".to_string(), Value::String(content.clone()));
            }
            NodeKind::EditableText { content, field } => {
                data.insert(field.clone(), Value::String(content.clone()));
            }
            NodeKind::BlockRef { block_id, .. } => {
                data.insert("id".to_string(), Value::String(block_id.clone()));
            }
            NodeKind::StateToggle { field, current, .. } => {
                data.insert(field.clone(), Value::String(current.clone()));
            }
            _ => {}
        }
    }
    if data.is_empty() { None } else { Some(data) }
}

/// Collect all nodes in the tree matching a predicate.
pub fn collect_nodes<'a>(
    node: &'a ViewModel,
    predicate: &dyn Fn(&ViewModel) -> bool,
) -> Vec<&'a ViewModel> {
    let mut result = Vec::new();
    collect_nodes_recursive(node, predicate, &mut result);
    result
}

fn collect_nodes_recursive<'a>(
    node: &'a ViewModel,
    predicate: &dyn Fn(&ViewModel) -> bool,
    result: &mut Vec<&'a ViewModel>,
) {
    if predicate(node) {
        result.push(node);
    }
    for child in node.children() {
        collect_nodes_recursive(child, predicate, result);
    }
}

/// Collect all EditableText nodes in the tree.
pub fn collect_editable_text_nodes(node: &ViewModel) -> Vec<&ViewModel> {
    collect_nodes(node, &|n| matches!(&n.kind, NodeKind::EditableText { .. }))
}

/// Collect all StateToggle nodes in the tree.
pub fn collect_state_toggle_nodes(node: &ViewModel) -> Vec<&ViewModel> {
    collect_nodes(node, &|n| matches!(&n.kind, NodeKind::StateToggle { .. }))
}

/// Count EditableText nodes that have operations but are missing triggers.
/// Returns (total_editable_with_ops, missing_triggers_count).
pub fn count_editables_missing_triggers(node: &ViewModel) -> (usize, usize) {
    let editables = collect_editable_text_nodes(node);
    let with_ops: Vec<_> = editables
        .iter()
        .filter(|n| !n.operations.is_empty())
        .collect();
    let missing = with_ops.iter().filter(|n| n.triggers.is_empty()).count();
    (with_ops.len(), missing)
}

/// Count nodes with widget_name == "error" in a ViewModel tree.
pub fn count_error_nodes(node: &ViewModel) -> usize {
    let self_count = if node.widget_name() == Some("error") {
        1
    } else {
        0
    };
    self_count + node.children().iter().map(count_error_nodes).sum::<usize>()
}

#[cfg(test)]
mod tests {
    use holon_api::Value;
    use holon_frontend::ViewModel;

    use super::*;

    #[test]
    fn identical_trees_produce_no_diffs() {
        let tree = ViewModel::layout(
            "col",
            vec![ViewModel::leaf("text", Value::String("hello".into()))],
        );
        assert!(tree_diff(&tree, &tree).is_empty());
    }

    #[test]
    fn widget_mismatch_detected() {
        let a = ViewModel::leaf("text", Value::String("x".into()));
        let b = ViewModel::leaf("badge", Value::String("x".into()));
        let diffs = tree_diff(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert!(matches!(diffs[0].kind, DiffKind::WidgetMismatch { .. }));
    }

    #[test]
    fn child_count_mismatch_detected() {
        let a = ViewModel::layout("row", vec![ViewModel::empty(), ViewModel::empty()]);
        let b = ViewModel::layout("row", vec![ViewModel::empty()]);
        let diffs = tree_diff(&a, &b);
        assert!(diffs.iter().any(|d| matches!(
            d.kind,
            DiffKind::ChildCountMismatch {
                actual: 2,
                expected: 1
            }
        )));
    }

    #[test]
    fn ordered_subset_happy() {
        let actual = vec!["a".into(), "c".into()];
        let expected = vec!["a".into(), "b".into(), "c".into()];
        let result = is_ordered_subset(&actual, &expected);
        assert!(result.is_subset);
    }

    #[test]
    fn ordered_subset_out_of_order() {
        let actual = vec!["c".into(), "a".into()];
        let expected = vec!["a".into(), "b".into(), "c".into()];
        let result = is_ordered_subset(&actual, &expected);
        assert!(!result.is_subset);
        assert!(!result.out_of_order.is_empty());
    }

    #[test]
    fn ordered_subset_missing() {
        let actual = vec!["a".into(), "z".into()];
        let expected = vec!["a".into(), "b".into()];
        let result = is_ordered_subset(&actual, &expected);
        assert!(!result.is_subset);
        assert_eq!(result.missing_from_expected, vec!["z"]);
    }
}

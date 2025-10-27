use std::fmt::Write;

use holon_frontend::DisplayNode;
use holon_frontend::display_node::NodeKind;

/// A single difference found between two DisplayNode trees.
#[derive(Debug, Clone)]
pub struct TreeDiff {
    pub path: String,
    pub kind: DiffKind,
}

#[derive(Debug, Clone)]
pub enum DiffKind {
    VariantMismatch {
        actual: &'static str,
        expected: &'static str,
    },
    WidgetMismatch {
        actual: String,
        expected: String,
    },
    ValueMismatch {
        actual: String,
        expected: String,
    },
    ChildCountMismatch {
        actual: usize,
        expected: usize,
    },
    DataKeyMissing {
        key: String,
    },
    DataValueMismatch {
        key: String,
        actual: String,
        expected: String,
    },
    OnlyInActual,
    OnlyInExpected,
}

impl std::fmt::Display for TreeDiff {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "at {}: ", self.path)?;
        match &self.kind {
            DiffKind::VariantMismatch { actual, expected } => {
                write!(f, "variant {actual} vs {expected}")
            }
            DiffKind::WidgetMismatch { actual, expected } => {
                write!(f, "widget \"{actual}\" vs \"{expected}\"")
            }
            DiffKind::ValueMismatch { actual, expected } => {
                write!(f, "value {actual} vs {expected}")
            }
            DiffKind::ChildCountMismatch { actual, expected } => {
                write!(f, "{actual} children vs {expected}")
            }
            DiffKind::DataKeyMissing { key } => write!(f, "missing key \"{key}\""),
            DiffKind::DataValueMismatch {
                key,
                actual,
                expected,
            } => write!(f, "data[\"{key}\"]: {actual} vs {expected}"),
            DiffKind::OnlyInActual => write!(f, "only in actual"),
            DiffKind::OnlyInExpected => write!(f, "only in expected"),
        }
    }
}

fn variant_name(node: &DisplayNode) -> &'static str {
    match &node.kind {
        NodeKind::Collection { .. } => "Collection",
        NodeKind::Layout { .. } => "Layout",
        NodeKind::Element { .. } => "Element",
        NodeKind::Leaf { .. } => "Leaf",
        NodeKind::BlockRef { .. } => "BlockRef",
        NodeKind::Error { .. } => "Error",
        NodeKind::Empty => "Empty",
    }
}

/// Recursively diff two DisplayNode trees. Returns all differences found.
pub fn tree_diff(actual: &DisplayNode, expected: &DisplayNode) -> Vec<TreeDiff> {
    let mut diffs = Vec::new();
    diff_recursive(actual, expected, &mut String::from("root"), &mut diffs);
    diffs
}

fn diff_recursive(
    actual: &DisplayNode,
    expected: &DisplayNode,
    path: &mut String,
    diffs: &mut Vec<TreeDiff>,
) {
    if variant_name(actual) != variant_name(expected) {
        diffs.push(TreeDiff {
            path: path.clone(),
            kind: DiffKind::VariantMismatch {
                actual: variant_name(actual),
                expected: variant_name(expected),
            },
        });
        return;
    }

    match (&actual.kind, &expected.kind) {
        (
            NodeKind::Collection {
                widget: aw,
                items: ai,
            },
            NodeKind::Collection {
                widget: ew,
                items: ei,
            },
        ) => {
            diff_widget(aw, ew, path, diffs);
            diff_children(ai, ei, path, diffs);
        }
        (
            NodeKind::Layout {
                widget: aw,
                children: ac,
            },
            NodeKind::Layout {
                widget: ew,
                children: ec,
            },
        ) => {
            diff_widget(aw, ew, path, diffs);
            diff_children(ac, ec, path, diffs);
        }
        (
            NodeKind::Element {
                widget: aw,
                data: ad,
                children: ac,
            },
            NodeKind::Element {
                widget: ew,
                data: ed,
                children: ec,
            },
        ) => {
            diff_widget(aw, ew, path, diffs);
            for (key, eval) in ed {
                match ad.get(key) {
                    None => diffs.push(TreeDiff {
                        path: path.clone(),
                        kind: DiffKind::DataKeyMissing { key: key.clone() },
                    }),
                    Some(aval) if aval != eval => diffs.push(TreeDiff {
                        path: path.clone(),
                        kind: DiffKind::DataValueMismatch {
                            key: key.clone(),
                            actual: format!("{aval:?}"),
                            expected: format!("{eval:?}"),
                        },
                    }),
                    _ => {}
                }
            }
            diff_children(ac, ec, path, diffs);
        }
        (
            NodeKind::Leaf {
                widget: aw,
                value: av,
            },
            NodeKind::Leaf {
                widget: ew,
                value: ev,
            },
        ) => {
            diff_widget(aw, ew, path, diffs);
            if av != ev {
                diffs.push(TreeDiff {
                    path: path.clone(),
                    kind: DiffKind::ValueMismatch {
                        actual: format!("{av:?}"),
                        expected: format!("{ev:?}"),
                    },
                });
            }
        }
        (
            NodeKind::BlockRef {
                block_id: ai,
                content: ac,
            },
            NodeKind::BlockRef {
                block_id: ei,
                content: ec,
            },
        ) => {
            if ai != ei {
                diffs.push(TreeDiff {
                    path: path.clone(),
                    kind: DiffKind::ValueMismatch {
                        actual: ai.clone(),
                        expected: ei.clone(),
                    },
                });
            }
            let prev_len = path.len();
            write!(path, "/content").unwrap();
            diff_recursive(ac, ec, path, diffs);
            path.truncate(prev_len);
        }
        (
            NodeKind::Error {
                widget: aw,
                message: am,
            },
            NodeKind::Error {
                widget: ew,
                message: em,
            },
        ) => {
            diff_widget(aw, ew, path, diffs);
            if am != em {
                diffs.push(TreeDiff {
                    path: path.clone(),
                    kind: DiffKind::ValueMismatch {
                        actual: am.clone(),
                        expected: em.clone(),
                    },
                });
            }
        }
        (NodeKind::Empty, NodeKind::Empty) => {}
        _ => unreachable!("variant mismatch already handled"),
    }
}

fn diff_widget(actual: &str, expected: &str, path: &str, diffs: &mut Vec<TreeDiff>) {
    if actual != expected {
        diffs.push(TreeDiff {
            path: path.to_string(),
            kind: DiffKind::WidgetMismatch {
                actual: actual.to_string(),
                expected: expected.to_string(),
            },
        });
    }
}

fn diff_children(
    actual: &[DisplayNode],
    expected: &[DisplayNode],
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

    let mut last_expected_idx: Option<usize> = None;

    for actual_id in actual_ids {
        match expected_ids.iter().position(|e| e == actual_id) {
            None => missing_from_expected.push(actual_id.clone()),
            Some(idx) => {
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

/// Assert two DisplayNode trees match structurally, with a nice error message on failure.
pub fn assert_display_trees_match(actual: &DisplayNode, expected: &DisplayNode, message: &str) {
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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use holon_api::Value;
    use holon_frontend::DisplayNode;

    use super::*;

    #[test]
    fn identical_trees_produce_no_diffs() {
        let tree = DisplayNode::layout(
            "col",
            vec![DisplayNode::leaf("text", Value::String("hello".into()))],
        );
        assert!(tree_diff(&tree, &tree).is_empty());
    }

    #[test]
    fn widget_mismatch_detected() {
        let a = DisplayNode::leaf("text", Value::String("x".into()));
        let b = DisplayNode::leaf("badge", Value::String("x".into()));
        let diffs = tree_diff(&a, &b);
        assert_eq!(diffs.len(), 1);
        assert!(matches!(diffs[0].kind, DiffKind::WidgetMismatch { .. }));
    }

    #[test]
    fn child_count_mismatch_detected() {
        let a = DisplayNode::layout("row", vec![DisplayNode::EMPTY, DisplayNode::EMPTY]);
        let b = DisplayNode::layout("row", vec![DisplayNode::EMPTY]);
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

    #[test]
    fn data_key_missing_detected() {
        let a = DisplayNode::element("block", HashMap::new(), vec![]);
        let b = DisplayNode::element(
            "block",
            HashMap::from([("id".into(), Value::String("x".into()))]),
            vec![],
        );
        let diffs = tree_diff(&a, &b);
        assert!(
            diffs
                .iter()
                .any(|d| matches!(&d.kind, DiffKind::DataKeyMissing { key } if key == "id"))
        );
    }
}

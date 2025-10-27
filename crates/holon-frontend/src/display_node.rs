use std::collections::HashMap;
use std::fmt::Write;

use holon_api::render_types::OperationWiring;
use holon_api::Value;
use serde::{Deserialize, Serialize};

/// A node in the shadow widget tree.
///
/// `kind` describes what kind of widget this is (collection, layout, leaf, etc.).
/// `operations` carries the operation bindings from the RenderExpr — used by
/// `ShadowDom` for input bubbling (e.g., matching key chords to operations).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DisplayNode {
    #[serde(flatten)]
    pub kind: NodeKind,

    /// Operations available at this node. Populated by the render interpreter's
    /// annotator from the RenderExpr's FunctionCall operations.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub operations: Vec<OperationWiring>,
}

/// The kind of widget this node represents.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum NodeKind {
    /// Collection iterating rows (list, tree, table, outline)
    Collection {
        widget: String,
        items: Vec<DisplayNode>,
    },
    /// Structural container (columns, row, section, col, block)
    Layout {
        widget: String,
        children: Vec<DisplayNode>,
    },
    /// One row's rendered content with its data
    Element {
        widget: String,
        data: HashMap<String, Value>,
        children: Vec<DisplayNode>,
    },
    /// Leaf displaying a resolved value (text, badge, checkbox, icon, spacer)
    Leaf { widget: String, value: Value },
    /// Nested block reference
    BlockRef {
        block_id: String,
        content: Box<DisplayNode>,
    },
    /// Error during rendering
    Error { widget: String, message: String },
    /// Empty (spacer, drop_zone, empty content)
    Empty,
}

impl PartialEq for DisplayNode {
    fn eq(&self, other: &Self) -> bool {
        self.kind == other.kind
    }
}

// ---------------------------------------------------------------------------
// Constructors — keep builder code concise
// ---------------------------------------------------------------------------

impl DisplayNode {
    pub fn collection(widget: impl Into<String>, items: Vec<DisplayNode>) -> Self {
        Self {
            kind: NodeKind::Collection {
                widget: widget.into(),
                items,
            },
            operations: vec![],
        }
    }

    pub fn layout(widget: impl Into<String>, children: Vec<DisplayNode>) -> Self {
        Self {
            kind: NodeKind::Layout {
                widget: widget.into(),
                children,
            },
            operations: vec![],
        }
    }

    pub fn element(
        widget: impl Into<String>,
        data: HashMap<String, Value>,
        children: Vec<DisplayNode>,
    ) -> Self {
        Self {
            kind: NodeKind::Element {
                widget: widget.into(),
                data,
                children,
            },
            operations: vec![],
        }
    }

    pub fn leaf(widget: impl Into<String>, value: Value) -> Self {
        Self {
            kind: NodeKind::Leaf {
                widget: widget.into(),
                value,
            },
            operations: vec![],
        }
    }

    pub fn block_ref(block_id: impl Into<String>, content: DisplayNode) -> Self {
        Self {
            kind: NodeKind::BlockRef {
                block_id: block_id.into(),
                content: Box::new(content),
            },
            operations: vec![],
        }
    }

    pub fn error(widget: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: NodeKind::Error {
                widget: widget.into(),
                message: message.into(),
            },
            operations: vec![],
        }
    }

    pub const EMPTY: Self = Self {
        kind: NodeKind::Empty,
        operations: vec![],
    };
}

// ---------------------------------------------------------------------------
// Tree traversal and display
// ---------------------------------------------------------------------------

impl DisplayNode {
    pub fn pretty_print(&self, indent: usize) -> String {
        let mut out = String::new();
        self.fmt_indent(&mut out, indent);
        out
    }

    fn fmt_indent(&self, out: &mut String, indent: usize) {
        let pad = "  ".repeat(indent);
        let ops_suffix = if self.operations.is_empty() {
            String::new()
        } else {
            format!(
                " [ops: {}]",
                self.operations
                    .iter()
                    .map(|o| o.descriptor.name.as_str())
                    .collect::<Vec<_>>()
                    .join(",")
            )
        };
        match &self.kind {
            NodeKind::Collection { widget, items } => {
                let _ = writeln!(out, "{pad}{widget} [{} items]{ops_suffix}", items.len());
                for item in items {
                    item.fmt_indent(out, indent + 1);
                }
            }
            NodeKind::Layout { widget, children } => {
                let _ = writeln!(out, "{pad}{widget}{ops_suffix}");
                for child in children {
                    child.fmt_indent(out, indent + 1);
                }
            }
            NodeKind::Element {
                widget,
                data,
                children,
            } => {
                let fields = format_data_inline(data);
                let _ = writeln!(out, "{pad}{widget} {{{fields}}}{ops_suffix}");
                for child in children {
                    child.fmt_indent(out, indent + 1);
                }
            }
            NodeKind::Leaf { widget, value } => {
                let val = value.to_display_string();
                let _ = writeln!(out, "{pad}{widget} {val:?}{ops_suffix}");
            }
            NodeKind::BlockRef { block_id, content } => {
                let _ = writeln!(out, "{pad}block_ref({block_id}){ops_suffix}");
                content.fmt_indent(out, indent + 1);
            }
            NodeKind::Error { widget, message } => {
                let _ = writeln!(out, "{pad}ERROR[{widget}]: {message}");
            }
            NodeKind::Empty => {
                let _ = writeln!(out, "{pad}(empty)");
            }
        }
    }

    /// Collect all entity IDs referenced in the tree (from Element data "id" fields
    /// and BlockRef block_ids), in depth-first order.
    pub fn collect_entity_ids(&self) -> Vec<String> {
        let mut ids = Vec::new();
        self.collect_ids_recursive(&mut ids);
        ids
    }

    fn collect_ids_recursive(&self, ids: &mut Vec<String>) {
        match &self.kind {
            NodeKind::Collection { items, .. } => {
                for item in items {
                    item.collect_ids_recursive(ids);
                }
            }
            NodeKind::Layout { children, .. } => {
                for child in children {
                    child.collect_ids_recursive(ids);
                }
            }
            NodeKind::Element { data, children, .. } => {
                if let Some(id) = data.get("id").and_then(|v| v.as_string()) {
                    ids.push(id.to_string());
                }
                for child in children {
                    child.collect_ids_recursive(ids);
                }
            }
            NodeKind::Leaf { .. } => {}
            NodeKind::BlockRef { block_id, content } => {
                ids.push(block_id.clone());
                content.collect_ids_recursive(ids);
            }
            NodeKind::Error { .. } | NodeKind::Empty => {}
        }
    }

    /// Get children of this node (items for Collection, children for Layout/Element,
    /// content for BlockRef).
    pub fn children(&self) -> &[DisplayNode] {
        match &self.kind {
            NodeKind::Collection { items, .. } => items,
            NodeKind::Layout { children, .. } | NodeKind::Element { children, .. } => children,
            NodeKind::BlockRef { content, .. } => std::slice::from_ref(content.as_ref()),
            NodeKind::Leaf { .. } | NodeKind::Error { .. } | NodeKind::Empty => &[],
        }
    }

    /// The widget name (e.g. "list", "row", "text").
    pub fn widget_name(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Collection { widget, .. }
            | NodeKind::Layout { widget, .. }
            | NodeKind::Element { widget, .. }
            | NodeKind::Leaf { widget, .. }
            | NodeKind::Error { widget, .. } => Some(widget),
            NodeKind::BlockRef { .. } => Some("block_ref"),
            NodeKind::Empty => None,
        }
    }

    /// Extract entity ID from Element data or BlockRef block_id.
    pub fn entity_id(&self) -> Option<&str> {
        match &self.kind {
            NodeKind::Element { data, .. } => data.get("id").and_then(|v| v.as_string()),
            NodeKind::BlockRef { block_id, .. } => Some(block_id.as_str()),
            _ => None,
        }
    }
}

fn format_data_inline(data: &HashMap<String, Value>) -> String {
    let mut pairs: Vec<_> = data
        .iter()
        .filter(|(k, _)| *k == "id" || *k == "content" || *k == "task_state")
        .collect();
    pairs.sort_by_key(|(k, _)| *k);
    pairs
        .iter()
        .map(|(k, v)| format!("{k}: {:?}", v.to_display_string()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pretty_print_nested() {
        let tree = DisplayNode::layout(
            "columns",
            vec![DisplayNode::collection(
                "list",
                vec![
                    DisplayNode::block_ref(
                        "a",
                        DisplayNode::element(
                            "block",
                            HashMap::from([
                                ("id".into(), Value::String("a".into())),
                                ("content".into(), Value::String("First".into())),
                            ]),
                            vec![],
                        ),
                    ),
                    DisplayNode::block_ref(
                        "b",
                        DisplayNode::element(
                            "block",
                            HashMap::from([
                                ("id".into(), Value::String("b".into())),
                                ("content".into(), Value::String("Second".into())),
                            ]),
                            vec![],
                        ),
                    ),
                ],
            )],
        );

        let output = tree.pretty_print(0);
        assert!(output.contains("columns"));
        assert!(output.contains("list [2 items]"));
        assert!(output.contains("block_ref(a)"));
        assert!(output.contains("block_ref(b)"));
    }

    #[test]
    fn collect_entity_ids_mixed() {
        let tree = DisplayNode::layout(
            "col",
            vec![
                DisplayNode::block_ref(
                    "ref-1",
                    DisplayNode::element(
                        "block",
                        HashMap::from([("id".into(), Value::String("inner-1".into()))]),
                        vec![],
                    ),
                ),
                DisplayNode::element(
                    "row",
                    HashMap::from([("id".into(), Value::String("row-1".into()))]),
                    vec![],
                ),
            ],
        );

        let ids = tree.collect_entity_ids();
        assert_eq!(ids, vec!["ref-1", "inner-1", "row-1"]);
    }

    #[test]
    fn children_accessor() {
        let list = DisplayNode::collection("list", vec![DisplayNode::EMPTY, DisplayNode::EMPTY]);
        assert_eq!(list.children().len(), 2);

        let leaf = DisplayNode::leaf("text", Value::String("hi".into()));
        assert!(leaf.children().is_empty());
    }

    #[test]
    fn entity_id_extraction() {
        let elem = DisplayNode::element(
            "block",
            HashMap::from([("id".into(), Value::String("abc".into()))]),
            vec![],
        );
        assert_eq!(elem.entity_id(), Some("abc"));

        let bref = DisplayNode::block_ref("xyz", DisplayNode::EMPTY);
        assert_eq!(bref.entity_id(), Some("xyz"));

        assert_eq!(DisplayNode::EMPTY.entity_id(), None);
    }
}

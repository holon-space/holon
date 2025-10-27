use std::collections::{BTreeSet, HashMap};

use holon_api::render_types::OperationWiring;

use crate::input::{InputAction, Key, WidgetInput};
use crate::navigation::{CollectionNavigator, ListNavigator, TreeNavigator};
use crate::view_model::{NodeKind, ViewModel};

/// Node index in the flattened ShadowDom.
pub type NodeIdx = usize;

/// A flattened, indexed view of a ViewModel tree that supports
/// efficient parent traversal and input bubbling.
///
/// Built once from a rendered ViewModel tree. Frontends construct a
/// ShadowDom after rendering, then call `bubble_input()` to handle
/// keyboard input without any framework-specific logic.
pub struct ShadowDom {
    nodes: Vec<ShadowNode>,
    /// entity_id → node index for initiating bubbling from a focused block.
    entity_index: HashMap<String, NodeIdx>,
    /// Key chord → operation name bindings.
    key_map: KeyMap,
}

#[allow(dead_code)] // entity_id + widget_name stored for future devtools/inspection
struct ShadowNode {
    parent: Option<NodeIdx>,
    entity_id: Option<String>,
    widget_name: Option<String>,
    operations: Vec<OperationWiring>,
    navigator: Option<Box<dyn CollectionNavigator>>,
    children: Vec<NodeIdx>,
}

/// Maps key chords to operation names.
///
/// During bubbling, when a `KeyChord` reaches a node with operations,
/// the ShadowDom checks if any operation's name matches a binding in the KeyMap.
#[derive(Debug, Clone, Default)]
pub struct KeyMap {
    bindings: HashMap<BTreeSet<Key>, String>,
}

impl KeyMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Bind a key chord to an operation name.
    pub fn bind(mut self, keys: &[Key], operation: impl Into<String>) -> Self {
        self.bindings
            .insert(keys.iter().cloned().collect(), operation.into());
        self
    }

    /// Look up which operation name a chord maps to.
    pub fn lookup(&self, keys: &BTreeSet<Key>) -> Option<&str> {
        self.bindings.get(keys).map(|s| s.as_str())
    }
}

impl ShadowDom {
    /// Build a ShadowDom from a rendered ViewModel tree.
    pub fn from_display_tree(root: &ViewModel, key_map: KeyMap) -> Self {
        let mut nodes = Vec::new();
        let mut entity_index = HashMap::new();
        flatten_recursive(root, None, &mut nodes, &mut entity_index);
        ShadowDom {
            nodes,
            entity_index,
            key_map,
        }
    }

    /// Try to handle an input starting from the node with the given entity_id.
    /// Bubbles up through parents until a handler consumes it or root is reached.
    /// Try to handle an input starting from the node with the given entity_id.
    /// Bubbles up through parents until a handler consumes it or root is reached.
    ///
    /// The `entity_id` identifies the focused block that initiated the input.
    /// When the input bubbles to a collection (for navigation) or to a parent
    /// with operations (for key chords), the originating entity_id is used:
    /// - Navigate: the collection's navigator resolves "what's next after entity_id"
    /// - KeyChord: the matched operation executes against entity_id
    pub fn bubble_input(&self, entity_id: &str, input: &WidgetInput) -> Option<InputAction> {
        let &start = self.entity_index.get(entity_id)?;
        let mut current = Some(start);

        while let Some(idx) = current {
            let node = &self.nodes[idx];
            if let Some(action) = self.try_handle(node, entity_id, input) {
                return Some(action);
            }
            current = node.parent;
        }

        None // bubbled through root, no handler
    }

    /// List all entity IDs in the ShadowDom.
    pub fn entity_ids(&self) -> Vec<&str> {
        self.entity_index.keys().map(|s| s.as_str()).collect()
    }

    /// Try to handle input at a specific node.
    /// `origin_id` is the entity that started the bubbling (the focused block).
    fn try_handle(
        &self,
        node: &ShadowNode,
        origin_id: &str,
        input: &WidgetInput,
    ) -> Option<InputAction> {
        match input {
            WidgetInput::Navigate { direction, hint } => {
                let navigator = node.navigator.as_ref()?;
                let target = navigator.navigate(origin_id, *direction, hint)?;
                Some(InputAction::Focus {
                    block_id: target.block_id,
                    placement: target.placement,
                })
            }
            WidgetInput::KeyChord { keys } => {
                let op_name = self.key_map.lookup(keys)?;
                let op = node
                    .operations
                    .iter()
                    .find(|ow| ow.descriptor.name == op_name)?;
                Some(InputAction::ExecuteOperation {
                    entity_name: op.descriptor.entity_name.to_string(),
                    operation: op.descriptor.clone(),
                    entity_id: origin_id.to_string(),
                })
            }
        }
    }
}

/// Flatten a ViewModel tree into a vec of ShadowNodes with parent indices.
fn flatten_recursive(
    node: &ViewModel,
    parent: Option<NodeIdx>,
    nodes: &mut Vec<ShadowNode>,
    entity_index: &mut HashMap<String, NodeIdx>,
) -> NodeIdx {
    let my_idx = nodes.len();

    let entity_id = node.entity_id().map(|s| s.to_string());
    if let Some(ref id) = entity_id {
        entity_index.insert(id.clone(), my_idx);
    }

    // Placeholder — children filled after recursion
    nodes.push(ShadowNode {
        parent,
        entity_id,
        widget_name: node.widget_name().map(|s| s.to_string()),
        operations: node.operations.clone(),
        navigator: None,
        children: vec![],
    });

    // Recurse into children
    let child_indices: Vec<NodeIdx> = node
        .children()
        .iter()
        .map(|child| flatten_recursive(child, Some(my_idx), nodes, entity_index))
        .collect();

    nodes[my_idx].children = child_indices.clone();

    // Build navigator for collection nodes
    let widget_name = node.widget_name().unwrap_or("");
    match widget_name {
        "list" | "tree" | "outline" | "table" | "query_result" => {
            let navigator = build_navigator(widget_name, node.children());
            nodes[my_idx].navigator = navigator;
        }
        _ => {}
    }

    my_idx
}

/// Build the appropriate CollectionNavigator for a collection widget.
fn build_navigator(widget: &str, items: &[ViewModel]) -> Option<Box<dyn CollectionNavigator>> {
    let ids = collect_direct_entity_ids(items);
    if ids.is_empty() {
        return None;
    }

    match widget {
        "tree" | "outline" => {
            // Build parent map from tree_item Layout nodes
            let mut dfs_order = Vec::new();
            let mut parent_map = HashMap::new();
            collect_tree_structure(items, None, &mut dfs_order, &mut parent_map);
            if dfs_order.is_empty() {
                return None;
            }
            Some(Box::new(TreeNavigator::from_dfs_and_parents(
                dfs_order, parent_map,
            )))
        }
        _ => {
            // list, table, query_result — linear navigation
            Some(Box::new(ListNavigator::new(ids)))
        }
    }
}

/// Collect entity IDs from direct children (one level deep).
fn collect_direct_entity_ids(items: &[ViewModel]) -> Vec<String> {
    items
        .iter()
        .filter_map(|item| item.entity_id().map(|s| s.to_string()))
        .collect()
}

/// Walk a tree/outline structure to extract DFS order and parent relationships.
/// Tree items with children are Layout { widget: "tree_item", children: [node, child1, child2, ...] }
fn collect_tree_structure(
    items: &[ViewModel],
    parent_id: Option<&str>,
    dfs_order: &mut Vec<String>,
    parent_map: &mut HashMap<String, String>,
) {
    for item in items {
        match &item.kind {
            NodeKind::TreeItem { children } => {
                // First child is the node itself, rest are children
                if let Some(first) = children.items.first() {
                    if let Some(id) = first.entity_id() {
                        dfs_order.push(id.to_string());
                        if let Some(pid) = parent_id {
                            parent_map.insert(id.to_string(), pid.to_string());
                        }
                        // Remaining children are sub-items
                        if children.items.len() > 1 {
                            collect_tree_structure(
                                &children.items[1..],
                                Some(id),
                                dfs_order,
                                parent_map,
                            );
                        }
                    }
                }
            }
            _ => {
                // Leaf item in tree (no children)
                if let Some(id) = item.entity_id() {
                    dfs_order.push(id.to_string());
                    if let Some(pid) = parent_id {
                        parent_map.insert(id.to_string(), pid.to_string());
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::navigation::{Boundary, CursorHint, CursorPlacement, NavDirection};
    use holon_api::Value;

    fn make_element(id: &str) -> ViewModel {
        ViewModel::element(
            "block",
            HashMap::from([("id".into(), Value::String(id.into()))]),
            vec![],
        )
    }

    #[test]
    fn bubble_navigate_in_list() {
        let tree = ViewModel::collection(
            "list",
            vec![make_element("a"), make_element("b"), make_element("c")],
        );
        let dom = ShadowDom::from_display_tree(&tree, KeyMap::new());

        let input = WidgetInput::Navigate {
            direction: NavDirection::Down,
            hint: CursorHint {
                column: 5,
                boundary: Boundary::Bottom,
            },
        };

        let action = dom.bubble_input("a", &input);
        match action {
            Some(InputAction::Focus {
                block_id,
                placement,
            }) => {
                assert_eq!(block_id, "b");
                assert_eq!(placement, CursorPlacement::FirstLine { column: 5 });
            }
            other => panic!("expected Focus, got {other:?}"),
        }

        // At boundary — "c" going Down → None
        assert!(dom.bubble_input("c", &input).is_none());
    }

    #[test]
    fn bubble_key_chord_to_operation() {
        use holon_api::render_types::{OperationDescriptor, OperationWiring, WidgetType};

        let op = OperationWiring {
            widget_type: WidgetType::Button,
            modified_param: "task_state".into(),
            descriptor: OperationDescriptor {
                entity_name: "block".into(),
                entity_short_name: "block".into(),
                id_column: "id".into(),
                name: "cycle_task_state".into(),
                display_name: "Cycle Task State".into(),
                description: "".into(),
                required_params: vec![],
                affected_fields: vec!["task_state".into()],
                param_mappings: vec![],
                precondition: None,
            },
        };

        // Block element with the operation attached
        let mut block = make_element("block-1");
        block.operations = vec![op];

        let tree = ViewModel::layout("row", vec![block]);

        let key_map = KeyMap::new().bind(&[Key::Cmd, Key::Enter], "cycle_task_state");
        let dom = ShadowDom::from_display_tree(&tree, key_map);

        let input = WidgetInput::chord(&[Key::Cmd, Key::Enter]);
        let action = dom.bubble_input("block-1", &input);

        match action {
            Some(InputAction::ExecuteOperation {
                entity_name,
                operation,
                entity_id,
            }) => {
                assert_eq!(entity_name, "block");
                assert_eq!(operation.name, "cycle_task_state");
                assert_eq!(entity_id, "block-1");
            }
            other => panic!("expected ExecuteOperation, got {other:?}"),
        }
    }

    #[test]
    fn unhandled_chord_returns_none() {
        let tree = ViewModel::collection("list", vec![make_element("a")]);
        let dom = ShadowDom::from_display_tree(&tree, KeyMap::new());

        let input = WidgetInput::chord(&[Key::Ctrl, Key::Shift, Key::Char('z')]);
        assert!(dom.bubble_input("a", &input).is_none());
    }
}

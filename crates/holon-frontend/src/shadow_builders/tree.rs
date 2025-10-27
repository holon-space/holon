use super::prelude::*;
use crate::render_interpreter::shared_tree_build;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let flat: Vec<(DisplayNode, usize)> = shared_tree_build(&ba);

    if flat.is_empty() {
        return DisplayNode::leaf("text", Value::String("[tree: no item_template]".into()));
    }

    let items = nest_by_depth(flat);
    DisplayNode::collection("tree", items)
}

/// Convert a flat depth-first `(node, depth)` list into a nested tree.
/// Each node's children are the subsequent nodes at depth+1 until the next
/// node at the same or lesser depth.
pub fn nest_by_depth(flat: Vec<(DisplayNode, usize)>) -> Vec<DisplayNode> {
    let mut result = Vec::new();
    let mut i = 0;
    nest_recursive(&flat, &mut i, 0, &mut result);
    result
}

fn nest_recursive(
    flat: &[(DisplayNode, usize)],
    i: &mut usize,
    current_depth: usize,
    out: &mut Vec<DisplayNode>,
) {
    while *i < flat.len() {
        let (_, depth) = &flat[*i];
        if *depth < current_depth {
            return;
        }
        let (node, _) = flat[*i].clone();
        *i += 1;

        // Collect children at depth+1
        let mut children = Vec::new();
        nest_recursive(flat, i, current_depth + 1, &mut children);

        if children.is_empty() {
            out.push(node);
        } else {
            // Wrap node + children into a Layout so the tree structure is visible
            let mut all = vec![node];
            all.extend(children);
            out.push(DisplayNode::layout("tree_item", all));
        }
    }
}

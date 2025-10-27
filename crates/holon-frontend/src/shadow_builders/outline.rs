use super::prelude::*;
use crate::shadow_builders::tree::nest_by_depth;
use crate::render_interpreter::shared_tree_build;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let flat: Vec<(DisplayNode, usize)> = shared_tree_build(&ba);

    if flat.is_empty() {
        return DisplayNode::error("outline", "no item_template");
    }

    let items = nest_by_depth(flat);
    DisplayNode::collection("outline", items)
}

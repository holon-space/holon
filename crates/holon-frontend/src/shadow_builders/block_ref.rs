use super::prelude::*;
use crate::render_interpreter::shared_block_ref_build;

pub fn build(ba: BA<'_>) -> DisplayNode {
    match shared_block_ref_build(&ba) {
        Ok(widget) => {
            let block_id = ba
                .args
                .get_positional_string(0)
                .unwrap_or("")
                .to_string();
            DisplayNode::block_ref(block_id, widget)
        }
        Err(msg) => DisplayNode::error("block_ref", msg),
    }
}

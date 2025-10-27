use super::prelude::*;
use crate::render_interpreter::shared_live_query_build;

pub fn build(ba: BA<'_>) -> DisplayNode {
    match shared_live_query_build(&ba) {
        Ok(widget) => widget,
        Err(msg) => DisplayNode::error("live_query", msg),
    }
}

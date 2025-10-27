use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let message = ba
        .args
        .get_string("message")
        .or_else(|| ba.args.get_positional_string(0))
        .unwrap_or("Unknown error")
        .to_string();

    DisplayNode::error("error", message)
}

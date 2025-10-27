use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let label = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("label"))
        .unwrap_or("")
        .to_string();

    DisplayNode::leaf("badge", Value::String(label))
}

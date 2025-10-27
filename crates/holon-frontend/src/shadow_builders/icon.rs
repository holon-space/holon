use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let name = ba
        .args
        .get_positional_string(0)
        .or(ba.args.get_string("name"))
        .unwrap_or("circle")
        .to_string();

    DisplayNode::leaf("icon", Value::String(name))
}

use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let content = ba
        .args
        .get_positional_string(0)
        .or_else(|| ba.args.get_string("content"))
        .unwrap_or("")
        .to_string();

    DisplayNode::leaf("editable_text", Value::String(content))
}

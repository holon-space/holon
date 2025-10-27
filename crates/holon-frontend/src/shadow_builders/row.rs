use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let mut children = Vec::new();

    if let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        children.push((ba.interpret)(template, ba.ctx));
    }

    for val in &ba.args.positional {
        if let Value::String(s) = val {
            children.push(DisplayNode::leaf("text", Value::String(s.clone())));
        }
    }

    DisplayNode::layout("row", children)
}

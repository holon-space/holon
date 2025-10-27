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

    DisplayNode::layout("block", children)
}

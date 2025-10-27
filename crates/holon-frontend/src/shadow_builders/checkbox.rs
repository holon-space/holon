use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    let checked = ba.args.get_bool("checked").unwrap_or(false);

    DisplayNode::leaf("checkbox", Value::Boolean(checked))
}

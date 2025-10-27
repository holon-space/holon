use super::prelude::*;

pub fn build(ba: BA<'_>) -> DisplayNode {
    if let Some(child_expr) = ba.args.positional_exprs.first() {
        (ba.interpret)(child_expr, ba.ctx)
    } else {
        DisplayNode::EMPTY
    }
}

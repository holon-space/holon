use super::prelude::*;

pub fn build(ba: BA<'_>) -> Div {
    let indent = (ba.ctx.depth as f32) * 29.0;

    let mut container = div().flex_col().pl(px(indent));

    if let Some(template) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        container = container.child((ba.interpret)(template, ba.ctx));
    }

    container
}

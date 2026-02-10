use super::prelude::*;

use crate::render::interpreter::interpret;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let indent = (ctx.depth as f32) * 29.0;

    let mut container = div().flex_col().pl(indent);

    if let Some(template) = args
        .get_template("item_template")
        .or(args.get_template("item"))
    {
        container = container.child(interpret(template, ctx));
    }

    container
}

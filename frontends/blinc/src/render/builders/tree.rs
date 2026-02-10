use super::prelude::*;

use crate::render::interpreter::interpret;
use holon_frontend::render_interpreter::{BuilderArgs, shared_tree_build};

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let ba = BuilderArgs {
        args,
        ctx,
        interpret: &|expr, ctx| interpret(expr, ctx),
    };
    let items = shared_tree_build(&ba);

    if items.is_empty() {
        return div().child(
            text("[tree: no item_template]")
                .size(12.0)
                .color(ThemeState::get().color(ColorToken::TextSecondary)),
        );
    }

    let mut container = div().flex_col().gap(2.0);
    for (widget, depth) in items {
        let indent = (depth as f32) * 16.0;
        container = container.child(div().flex_row().pl(indent).child(widget));
    }
    container
}

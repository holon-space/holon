use super::prelude::*;

use crate::render::context::BlincExt;
use crate::render::interpreter::interpret;
use holon_frontend::render_interpreter::{BuilderArgs, shared_tree_build};

/// outline(parent_id:parent_id, sortkey:sort_key, item_template:(render_block this))
///
/// Renders data as an indented hierarchical list using parent-child relationships.
/// Now delegates to `shared_tree_build` — same logic as `tree`.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let ext = ctx.ext.clone();
    let ba = BuilderArgs {
        args,
        ctx: &ctx.ctx,
        interpret: &|expr, inner_ctx| {
            let wrapped = RenderContext { ctx: inner_ctx.clone(), ext: ext.clone() };
            interpret(expr, &wrapped)
        },
    };
    let items = shared_tree_build(&ba);

    if items.is_empty() {
        return div().child(
            text("[outline: no item_template]")
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

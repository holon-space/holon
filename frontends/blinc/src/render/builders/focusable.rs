use super::prelude::*;

use super::operation_helpers::get_row_id;
use crate::render::interpreter::interpret;

/// focusable(child_expr, block_id:"...") — click-to-focus wrapper.
///
/// On click, stores the block id in the shared focused_block_id state.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let child = if let Some(child_expr) = args.positional_exprs.first() {
        interpret(child_expr, ctx)
    } else {
        div()
    };

    let block_id = args
        .get_string("block_id")
        .map(|s| s.to_string())
        .or_else(|| get_row_id(ctx));

    let Some(block_id) = block_id else {
        return child;
    };

    let focused = ctx.ext.focused_block_id.clone();
    child.on_click(move |_| {
        if let Some(ref state) = focused {
            let bid = block_id.clone();
            state.update_rebuild(|_| Some(bid));
        }
    })
}

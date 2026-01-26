use super::prelude::*;

use crate::render::interpreter::interpret;

/// draggable(child_expr, on:"drag"|"longpress") — drag wrapper stub.
///
/// Renders child as-is until Blinc has drag primitives.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    tracing::debug!("draggable: drag not yet supported in Blinc, rendering child only");

    if let Some(child_expr) = args.positional_exprs.first() {
        interpret(child_expr, ctx)
    } else {
        div()
    }
}

use super::prelude::*;

use crate::render::interpreter::interpret;

/// selectable(child_expr, action:(entity.operation param1:val1 ...))
///
/// Wraps a child element with an on_click handler that dispatches an operation.
pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> Div {
    let child = if let Some(child_expr) = args.positional_exprs.first() {
        interpret(child_expr, ctx)
    } else {
        div()
    };

    let action_expr = match args.get_template("action") {
        Some(expr) => expr,
        None => return child,
    };

    if let Some(action) = holon_frontend::operations::parse_action_expr(action_expr, ctx.row()) {
        let session = ctx.session().clone();
        let handle = ctx.runtime_handle().clone();
        return child.on_click(move |_| {
            holon_frontend::operations::dispatch_operation(
                &handle,
                &session,
                action.entity_name.clone(),
                action.op_name.clone(),
                action.params.clone(),
            );
        });
    }

    child
}

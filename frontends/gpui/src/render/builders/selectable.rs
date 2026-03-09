use std::collections::HashMap;

use super::prelude::*;
use holon_api::render_eval::eval_to_value;

pub fn build(ba: BA<'_>) -> Div {
    let child = if let Some(child_expr) = ba.args.positional_exprs.first() {
        (ba.interpret)(child_expr, ba.ctx)
    } else {
        div()
    };

    let action_expr = match ba.args.get_template("action") {
        Some(expr) => expr,
        None => return child,
    };

    if let holon_api::render_types::RenderExpr::FunctionCall {
        name,
        args: action_args,
        ..
    } = action_expr
    {
        let parts: Vec<&str> = name.split('.').collect();
        if parts.len() == 2 {
            let entity_name = parts[0].to_string();
            let op_name = parts[1].to_string();

            let mut params = HashMap::new();
            for arg in action_args {
                if let Some(ref param_name) = arg.name {
                    let value = eval_to_value(&arg.value, ba.ctx.row());
                    params.insert(param_name.clone(), value);
                }
            }

            let session = ba.ctx.session.clone();
            let handle = ba.ctx.runtime_handle.clone();
            return child.cursor_pointer().on_mouse_down(
                gpui::MouseButton::Left,
                move |_, _, _| {
                    holon_frontend::operations::dispatch_operation(
                        &handle,
                        &session,
                        entity_name.clone(),
                        op_name.clone(),
                        params.clone(),
                    );
                },
            );
        }
    }

    child
}

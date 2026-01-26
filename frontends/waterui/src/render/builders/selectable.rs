use holon_api::render_eval::{self as render_eval};
use holon_api::render_types::RenderExpr;

use super::prelude::*;

pub fn build(ba: BA) -> AnyView {
    let child = if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        (ba.interpret)(tmpl, ba.ctx)
    } else if let Some(first) = ba.args.positional_exprs.first() {
        (ba.interpret)(first, ba.ctx)
    } else {
        AnyView::new(text("[selectable]").size(12.0))
    };

    let action_expr = match ba.args.get_template("action") {
        Some(expr) => expr,
        None => return child,
    };

    if let RenderExpr::FunctionCall {
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
                    let value = render_eval::eval_to_value(&arg.value, ba.ctx.row());
                    params.insert(param_name.clone(), value);
                }
            }

            let session = ba.ctx.session.clone();
            let handle = ba.ctx.runtime_handle.clone();
            return AnyView::new(child.on_tap(move || {
                holon_frontend::operations::dispatch_operation(
                    &handle,
                    &session,
                    entity_name.clone(),
                    op_name.clone(),
                    params.clone(),
                );
            }));
        }
    }

    child
}

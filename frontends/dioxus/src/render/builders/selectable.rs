use std::collections::HashMap;

use super::prelude::*;
use holon_api::render_eval;
use holon_api::render_types::RenderExpr;

pub fn build(ba: BA<'_>) -> Element {
    let child = if let Some(tmpl) = ba
        .args
        .get_template("item_template")
        .or(ba.args.get_template("item"))
    {
        (ba.interpret)(tmpl, ba.ctx)
    } else if let Some(first) = ba.args.positional_exprs.first() {
        (ba.interpret)(first, ba.ctx)
    } else {
        rsx! { span { font_size: "12px", "[selectable]" } }
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
            return rsx! {
                div {
                    cursor: "pointer",
                    onclick: move |_| {
                        crate::operations::dispatch_operation(
                            &session,
                            entity_name.clone(),
                            op_name.clone(),
                            params.clone(),
                        );
                    },
                    {child}
                }
            };
        }
    }

    child
}

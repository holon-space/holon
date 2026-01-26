use std::collections::HashMap;
use std::sync::Arc;

use super::prelude::*;
use holon_api::render_eval::eval_to_value;

pub fn build(args: &ResolvedArgs, ctx: &RenderContext) -> PlyWidget {
    let child = if let Some(child_expr) = args.positional_exprs.first() {
        interpret(child_expr, ctx)
    } else {
        empty_widget()
    };

    let action_expr = match args.get_template("action") {
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
                    let value = eval_to_value(&arg.value, ctx.row());
                    params.insert(param_name.clone(), value);
                }
            }

            let session = ctx.session.clone();
            let handle = ctx.runtime_handle.clone();
            let params = Arc::new(params);

            return Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
                let session = session.clone();
                let handle = handle.clone();
                let entity_name = entity_name.clone();
                let op_name = op_name.clone();
                let params = Arc::clone(&params);
                ui.element()
                    .on_press(move |_id, _pointer| {
                        holon_frontend::operations::dispatch_operation(
                            &handle,
                            &session,
                            entity_name.clone(),
                            op_name.clone(),
                            (*params).clone(),
                        );
                    })
                    .children(|ui| {
                        child(ui);
                    });
            });
        }
    }

    child
}

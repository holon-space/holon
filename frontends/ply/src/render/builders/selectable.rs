use std::sync::Arc;

use super::prelude::*;

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

    if let Some(action) = holon_frontend::operations::parse_action_expr(action_expr, ctx.row()) {
        let session = ctx.session().clone();
        let handle = ctx.runtime_handle().clone();
        let params = Arc::new(action.params);

        return Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
            let session = session.clone();
            let handle = handle.clone();
            let entity_name = action.entity_name.clone();
            let op_name = action.op_name.clone();
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

    child
}

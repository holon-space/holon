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

    if let Some(intent) = holon_frontend::operations::parse_action_expr(action_expr, ctx.row()) {
        let services = ctx.services.clone();

        return Box::new(move |ui: &mut ply_engine::Ui<'_, ()>| {
            let services = services.clone();
            let intent = intent.clone();
            ui.element()
                .on_press(move |_id, _pointer| {
                    services.dispatch_intent(intent.clone());
                })
                .children(|ui| {
                    child(ui);
                });
        });
    }

    child
}

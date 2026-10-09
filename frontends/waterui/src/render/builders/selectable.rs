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

    let action =
        match holon_frontend::operations::parse_action_expr(action_expr, ba.services, ba.ctx) {
            Ok(action) => action,
            Err(e) => {
                return AnyView::new(
                    text(e.to_string())
                        .size(12.0)
                        .foreground(Color::srgb_hex("#FF0000")),
                );
            }
        };
    if let Some(action) = action {
        let session = ba.ctx.session.clone();
        let spawner: std::sync::Arc<dyn holon_api::spawner::Spawner> = std::sync::Arc::new(
            holon_api::spawner::TokioSpawner::new(ba.ctx.runtime_handle.clone()),
        );
        return AnyView::new(child.on_tap(move || {
            holon_frontend::operations::dispatch_operation(
                &spawner,
                &session,
                action.entity_name.clone(),
                action.op_name.clone(),
                action.params.clone(),
            );
        }));
    }

    child
}
